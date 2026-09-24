use crate::crypt::session::SessionKey;
use anyhow::{Context, Result, bail};
use std::{net::Ipv4Addr, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use wow_login_messages::{
    Message,
    all::{CMD_AUTH_LOGON_CHALLENGE_Client, Locale, Os, Platform, ProtocolVersion, Version},
    helper::tokio_expect_client_message,
    version_8::{
        AccountFlag, CMD_AUTH_LOGON_CHALLENGE_Server, CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag,
        CMD_AUTH_LOGON_PROOF_Client, CMD_AUTH_LOGON_PROOF_Server, CMD_REALM_LIST_Client,
        RealmCategory, RealmType, opcodes::ServerOpcodeMessage,
    },
};
use wow_srp::{
    GENERATOR, LARGE_SAFE_PRIME_LITTLE_ENDIAN, PublicKey, client::SrpClientChallenge,
    normalized_string::NormalizedString, server::SrpVerifier,
};

#[derive(Clone, Debug)]
pub struct LoginIdentity {
    pub account: String,
    pub configured: bool,
}
#[derive(Clone, Debug)]
pub struct ConfiguredCredential {
    pub account: String,
    pub password: String,
}
#[derive(Clone, Debug)]
pub struct AuthenticatedContext {
    pub account: String,
    pub session_key: SessionKey,
}
#[derive(Clone, Debug)]
pub struct UpstreamAuthConfig {
    pub host: String,
    pub port: u16,
    pub realm_name: String,
    pub world_host: String,
    pub world_port: u16,
    pub account: String,
}
#[derive(Clone, Debug)]
pub struct UpstreamLogin {
    pub session_key: SessionKey,
    pub realm_id: u32,
    pub realm_name: String,
    pub world_host: String,
    pub world_port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthError {
    Timeout,
    Transport(String),
    ChallengeRejected,
    ProofRejected,
    RealmListFailed,
    RealmMissing(String),
}

pub async fn with_handshake_timeout<F, T>(timeout: Duration, future: F) -> Result<T, AuthError>
where
    F: std::future::Future<Output = Result<T, AuthError>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| AuthError::Timeout)?
}

pub async fn upstream_login(config: &UpstreamAuthConfig, password: &str) -> Result<UpstreamLogin> {
    let addr = format_endpoint(&config.host, config.port);
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("failed to connect upstream auth {addr}"))?;
    let username = config.account.to_uppercase();
    CMD_AUTH_LOGON_CHALLENGE_Client {
        protocol_version: ProtocolVersion::Eight,
        version: Version {
            major: 3,
            minor: 3,
            patch: 5,
            build: 12340,
        },
        platform: Platform::X86,
        os: Os::Windows,
        locale: Locale::EnGb,
        utc_timezone_offset: 0,
        client_ip_address: Ipv4Addr::LOCALHOST,
        account_name: username.clone(),
    }
    .tokio_write(&mut stream)
    .await?;
    let challenge =
        ServerOpcodeMessage::tokio_read_protocol(&mut stream, ProtocolVersion::Eight).await?;
    let (generator, prime, salt, server_public) = match challenge {
        ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
            CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                generator,
                large_safe_prime,
                salt,
                server_public_key,
                ..
            },
        ) => (generator, large_safe_prime, salt, server_public_key),
        other => bail!("upstream auth challenge failed: {other:?}"),
    };
    let generator = *generator.first().context("empty SRP generator")?;
    let prime: [u8; 32] = prime
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid SRP prime length"))?;
    let server_public = PublicKey::from_le_bytes(server_public)?;
    let client = SrpClientChallenge::new(
        NormalizedString::new(&username)?,
        NormalizedString::new(password)?,
        generator,
        prime,
        server_public,
        salt,
    );
    CMD_AUTH_LOGON_PROOF_Client {
        client_public_key: *client.client_public_key(),
        client_proof: *client.client_proof(),
        crc_hash: [0; 20],
        telemetry_keys: vec![],
        security_flag: Default::default(),
    }
    .tokio_write(&mut stream)
    .await?;
    let proof =
        ServerOpcodeMessage::tokio_read_protocol(&mut stream, ProtocolVersion::Eight).await?;
    let server_proof = match proof {
        ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
            server_proof,
            ..
        }) => server_proof,
        other => bail!("upstream auth proof failed: {other:?}"),
    };
    let client = client.verify_server_proof(server_proof)?;
    CMD_REALM_LIST_Client {}.tokio_write(&mut stream).await?;
    let realms = match ServerOpcodeMessage::tokio_read_protocol(&mut stream, ProtocolVersion::Eight)
        .await?
    {
        ServerOpcodeMessage::CMD_REALM_LIST(list) => list.realms,
        other => bail!("expected upstream realm list, got {other:?}"),
    };
    let realm = realms
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(&config.realm_name))
        .with_context(|| format!("configured realm {:?} was not returned", config.realm_name))?;
    let port = parse_advertised_port(&realm.address, config.world_port);
    Ok(UpstreamLogin {
        session_key: *client.session_key(),
        realm_id: u32::from(realm.realm_id),
        realm_name: realm.name.clone(),
        world_host: config.world_host.clone(),
        world_port: port,
    })
}

pub async fn terminate_configured_downstream<S>(
    stream: &mut S,
    credential: &ConfiguredCredential,
) -> Result<AuthenticatedContext>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let username = NormalizedString::new(&credential.account.to_uppercase())?;
    let password = NormalizedString::new(&credential.password)?;
    let proof = SrpVerifier::from_username_and_password(username, password).into_proof();
    CMD_AUTH_LOGON_CHALLENGE_Server::Success {
        server_public_key: *proof.server_public_key(),
        generator: vec![GENERATOR],
        large_safe_prime: LARGE_SAFE_PRIME_LITTLE_ENDIAN.into(),
        salt: *proof.salt(),
        crc_salt: [0; 16],
        security_flag: CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag::empty(),
    }
    .tokio_write(&mut *stream)
    .await?;
    let reply = tokio_expect_client_message::<CMD_AUTH_LOGON_PROOF_Client, _>(&mut *stream).await?;
    let public_key = PublicKey::from_le_bytes(reply.client_public_key)?;
    let (server, server_proof) = proof
        .into_server(public_key, reply.client_proof)
        .map_err(|_| anyhow::anyhow!("downstream SRP proof verification failed"))?;
    CMD_AUTH_LOGON_PROOF_Server::Success {
        account_flag: AccountFlag::empty(),
        server_proof,
        hardware_survey_id: 0,
        unknown: 0,
    }
    .tokio_write(&mut *stream)
    .await?;
    Ok(AuthenticatedContext {
        account: credential.account.to_uppercase(),
        session_key: *server.session_key(),
    })
}

fn parse_advertised_port(address: &str, fallback: u16) -> u16 {
    let a = address.trim();
    if a.parse::<std::net::IpAddr>().is_ok() {
        return fallback;
    }
    if let Ok(socket) = a.parse::<std::net::SocketAddr>() {
        return socket.port();
    }
    a.rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(fallback)
}
pub fn format_endpoint(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

// Keep these imports pinned by the supported login message version. They are also
// useful to callers constructing configured realm responses.
#[allow(dead_code)]
fn _realm_type_markers(_: RealmType, _: RealmCategory) {}
