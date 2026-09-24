import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]
SERVICE_SCRIPT = REPO_ROOT / "tools" / "wow-bot-service.sh"


class WowBotServiceScriptTests(unittest.TestCase):
    def setUp(self):
        if not Path("/bin/zsh").is_file():
            self.skipTest("wow-bot-service.sh requires zsh")

        self.temp = tempfile.TemporaryDirectory(prefix="wow-bot-service-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        (self.root / "tools").mkdir()
        self.script = self.root / "tools" / "wow-bot-service.sh"
        shutil.copy2(SERVICE_SCRIPT, self.script)
        self.bin_dir = Path(self.temp.name) / "bin"
        self.bin_dir.mkdir()
        self.tmp_dir = Path(self.temp.name) / "tmp"
        self.tmp_dir.mkdir()
        self.state_file = Path(self.temp.name) / "launchctl-state"
        self.cargo_log = Path(self.temp.name) / "cargo.log"
        self.write_command("cargo", self.fake_cargo())
        self.write_command("launchctl", self.fake_launchctl())

    def write_command(self, name, contents):
        command = self.bin_dir / name
        command.write_text(contents)
        command.chmod(0o755)

    @staticmethod
    def fake_cargo():
        return """#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$FAKE_CARGO_LOG"
mkdir -p target/debug
printf '#!/bin/sh\\nexit 0\\n' > target/debug/wow-bot-supervisor
printf '#!/bin/sh\\nexit 0\\n' > target/debug/wow-bot-worker
chmod +x target/debug/wow-bot-supervisor target/debug/wow-bot-worker
"""

    @staticmethod
    def fake_launchctl():
        return """#!/bin/sh
set -eu
case "$1" in
  print)
    if [ -f "$FAKE_LAUNCHCTL_STATE" ]; then
      printf 'pid = 4242\\n'
      exit 0
    fi
    exit 1
    ;;
  bootstrap)
    touch "$FAKE_LAUNCHCTL_STATE"
    ;;
  bootout)
    rm -f "$FAKE_LAUNCHCTL_STATE"
    ;;
  *)
    exit 2
    ;;
esac
"""

    def run_service(self, action):
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.bin_dir}{os.pathsep}{env['PATH']}",
                "TMPDIR": str(self.tmp_dir),
                "FAKE_CARGO_LOG": str(self.cargo_log),
                "FAKE_LAUNCHCTL_STATE": str(self.state_file),
            }
        )
        return subprocess.run(
            ["/bin/zsh", str(self.script), action],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_start_writes_expected_plist_and_is_idempotent(self):
        started = self.run_service("start")
        self.assertEqual(started.returncode, 0, started.stderr)
        self.assertIn("Started wow-bot as a background service.", started.stdout)
        self.assertEqual(self.cargo_log.read_text().splitlines(), ["build -p wow-bot-supervisor -p wow-bot-worker"])

        plist_path = self.root / "wow-bot-data/service/com.wowbot.supervisor.plist"
        with plist_path.open("rb") as source:
            job = plistlib.load(source)
        service_bin = self.root / "tmp" / f"bin-{os.getuid()}"
        service_dir = self.root / "tmp" / "service"
        self.assertEqual(job["Label"], "com.wowbot.supervisor")
        self.assertEqual(job["WorkingDirectory"], str(self.root))
        self.assertEqual(
            job["ProgramArguments"],
            [str(service_bin / "wow-bot-supervisor"), "--worker-bin", str(service_bin / "wow-bot-worker"), "--config", str(self.root.parent / "config.toml")],
        )
        self.assertEqual(job["EnvironmentVariables"]["WOW_BOT_HOME"], str(self.root / "wow-bot-data"))
        self.assertEqual(job["EnvironmentVariables"]["TMPDIR"], str(self.root / "tmp"))
        self.assertEqual(job["EnvironmentVariables"]["RUST_LOG"], "info")
        self.assertEqual(job["StandardOutPath"], str(self.root / "tmp/logs/supervisor-console.log"))
        self.assertEqual(job["StandardErrorPath"], job["StandardOutPath"])
        self.assertTrue((service_dir / f"com.wowbot.supervisor.{os.getuid()}.plist").is_file())
        self.assertTrue((service_bin / "wow-bot-supervisor").is_file())
        self.assertTrue((service_bin / "wow-bot-worker").is_file())

        repeated = self.run_service("start")
        self.assertEqual(repeated.returncode, 0, repeated.stderr)
        self.assertIn("wow-bot service is already loaded.", repeated.stdout)
        self.assertEqual(len(self.cargo_log.read_text().splitlines()), 1)

    def test_stop_unloads_service_and_stopped_status_fails(self):
        self.assertEqual(self.run_service("start").returncode, 0)

        stopped = self.run_service("stop")
        self.assertEqual(stopped.returncode, 0, stopped.stderr)
        self.assertIn("Stopped wow-bot service.", stopped.stdout)
        self.assertFalse(self.state_file.exists())

        status = self.run_service("status")
        self.assertEqual(status.returncode, 1)
        self.assertIn("wow-bot service is stopped.", status.stdout)

    def test_unknown_action_returns_usage_error(self):
        result = self.run_service("reload")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
