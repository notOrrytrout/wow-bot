use anyhow::{Context, Result, bail};
use std::{env, time::Duration};
use tokio::{net::TcpStream, sync::mpsc};
use tracing_subscriber::EnvFilter;
use wow_control_proto::{
    ProxyToWorker, SupervisorCommand, SupervisorWire, WorkerEvent, WorkerWire,
    net::{read_frame, write_frame},
};
use wow_domain::{ActivationStage, LaneId, Mission, PauseReasons, WorkerGeneration};
use wow_engine::{
    activity::ActivityArbiter,
    lane::{LaneEngine, LaneMessage, LaneState},
};
use wow_state::AuthoritativeState;

#[derive(Debug)]
struct Args {
    lane: LaneId,
    generation: WorkerGeneration,
    control: String,
    maps_dir: std::path::PathBuf,
}

fn args() -> Result<Args> {
    let mut lane = None;
    let mut generation = None;
    let mut control = None;
    let mut maps_dir = None;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--lane" => {
                lane = Some(LaneId(
                    it.next().context("--lane requires a value")?.parse()?,
                ))
            }
            "--generation" => {
                generation = Some(WorkerGeneration(
                    it.next()
                        .context("--generation requires a value")?
                        .parse()?,
                ))
            }
            "--control" => control = Some(it.next().context("--control requires a value")?),
            "--maps-dir" => {
                maps_dir = Some(std::path::PathBuf::from(
                    it.next().context("--maps-dir requires a value")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "usage: wow-bot-worker --lane <id> --generation <n> --control <host:port> --maps-dir <path>"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(Args {
        lane: lane.context("missing --lane")?,
        generation: generation.context("missing --generation")?,
        control: control.context("missing --control")?,
        maps_dir: maps_dir.context("missing --maps-dir")?,
    })
}

fn initial_state(args: &Args) -> LaneState {
    LaneState {
        lane: args.lane,
        worker: args.generation,
        ownership: Default::default(),
        movement_epoch: Default::default(),
        mission: Mission::idle(),
        mission_revision: Default::default(),
        permission_revision: Default::default(),
        pause: PauseReasons::STARTUP_GATE,
        activation: ActivationStage::Observe,
        authoritative: AuthoritativeState::default(),
        activity: ActivityArbiter::default(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = args()?;
    let stream = TcpStream::connect(&args.control)
        .await
        .with_context(|| format!("failed to connect worker control socket {}", args.control))?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();

    write_frame(
        &mut writer,
        &WorkerWire::Hello {
            lane: args.lane,
            generation: args.generation,
        },
    )
    .await?;
    write_frame(
        &mut writer,
        &WorkerWire::Event(WorkerEvent::Ready {
            lane: args.lane,
            generation: args.generation,
        }),
    )
    .await?;

    let (lane_tx, lane_rx) = mpsc::channel(256);
    let (proxy_tx, mut proxy_rx) = mpsc::channel(256);
    let terrain = wow_navigation::TerrainSampler::new(&args.maps_dir).map_err(|e| {
        anyhow::anyhow!(
            "failed to initialize terrain sampler at {}: {e:?}",
            args.maps_dir.display()
        )
    })?;
    let runtime_root = args
        .maps_dir
        .parent()
        .context("maps directory has no AzerothCore runtime root")?;
    let mmaps_dir = runtime_root.join("mmaps");
    let movement_controller =
        wow_navigation::MovementController::new_with_mmaps(terrain, &mmaps_dir).map_err(|e| {
            anyhow::anyhow!(
                "failed to initialize Detour/MMAP navigation at {}: {e:?}",
                mmaps_dir.display()
            )
        })?;
    let engine = LaneEngine::new(initial_state(&args), lane_rx, proxy_tx)
        .with_movement_controller(movement_controller);
    let engine_task = tokio::spawn(engine.run());

    let lane = args.lane;
    let generation = args.generation;
    let writer_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    write_frame(&mut writer, &WorkerWire::Event(WorkerEvent::Heartbeat { lane, generation })).await?;
                }
                message = proxy_rx.recv() => {
                    let Some(message) = message else { break; };
                    write_frame(&mut writer, &WorkerWire::Proxy(message)).await?;
                }
            }
        }
        Result::<()>::Ok(())
    });

    while let Ok(message) = read_frame::<_, SupervisorWire>(&mut reader).await {
        match message {
            SupervisorWire::Proxy(ProxyToWorker::Observation(observation)) => {
                if lane_tx
                    .send(LaneMessage::Observation(observation))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            SupervisorWire::Proxy(ProxyToWorker::OwnershipChanged(owner)) => {
                if lane_tx
                    .send(LaneMessage::Ownership {
                        generation: owner.generation,
                        movement: owner.movement_epoch,
                        bot_allowed: owner.bot_allowed,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            SupervisorWire::Proxy(ProxyToWorker::MovementFence(epoch)) => {
                if lane_tx
                    .send(LaneMessage::MovementFence(epoch))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            SupervisorWire::Proxy(ProxyToWorker::WorkerGeneration(current)) => {
                if current != generation {
                    break;
                }
            }
            SupervisorWire::Proxy(ProxyToWorker::Shutdown) => break,
            SupervisorWire::Proxy(ProxyToWorker::ActionResult { action, result }) => {
                tracing::info!(?action, ?result, "gameplay action transport result");
            }
            SupervisorWire::Proxy(ProxyToWorker::SessionState {
                connected,
                in_world,
            }) => {
                tracing::info!(
                    connected,
                    in_world,
                    "configured session state received from proxy"
                );
                // A live socket is not proof of an entered character world. Wait for
                // SMSG_LOGIN_VERIFY_WORLD from AzerothCore before creating EnteredWorld.
                if !connected || !in_world {
                    if lane_tx
                        .send(LaneMessage::Observation(
                            wow_state::ProtocolObservation::LeftWorld,
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            SupervisorWire::Command(SupervisorCommand::ReplaceMission {
                lane: target,
                mission,
            }) if target == lane => {
                if lane_tx
                    .send(LaneMessage::ReplaceMission(mission))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            SupervisorWire::Command(SupervisorCommand::SetPause {
                lane: target,
                reasons,
            }) if target == lane => {
                if lane_tx.send(LaneMessage::SetPause(reasons)).await.is_err() {
                    break;
                }
            }
            SupervisorWire::Command(SupervisorCommand::UpdatePause {
                lane: target,
                set,
                clear,
            }) if target == lane => {
                if lane_tx
                    .send(LaneMessage::UpdatePause { set, clear })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            SupervisorWire::Command(SupervisorCommand::StopLane(target)) if target == lane => break,
            SupervisorWire::Command(SupervisorCommand::Shutdown) => break,
            SupervisorWire::Command(SupervisorCommand::StartLane(target)) if target == lane => {
                let _ = lane_tx
                    .send(LaneMessage::SetPause(PauseReasons::empty()))
                    .await;
                let _ = lane_tx
                    .send(LaneMessage::SetActivation(ActivationStage::Act))
                    .await;
            }
            SupervisorWire::Command(_) => {}
        }
    }

    let _ = lane_tx.send(LaneMessage::Shutdown).await;
    engine_task.await?;
    writer_task.abort();
    Ok(())
}
