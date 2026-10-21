"""Run local round-4 regressions without changing production test dependencies.

python test/round4_regressions/run.py ../axtest/round4-regressions-windows
cd ../axtest/round4-regressions-windows
cargo test --workspace

For Linux, extract the emitted .tar onto a native Linux filesystem before running.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tarfile


root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument("destination", type=Path)
args = parser.parse_args()
destination = args.destination.resolve()
assert destination.is_relative_to(root.parent / "axtest"), "snapshot must stay under axtest"
assert not destination.exists(), "use a fresh snapshot directory"
files = set(subprocess.check_output(["git", "ls-files"], cwd=root, text=True).splitlines())
files.update(p.relative_to(root).as_posix() for p in (root / "crates").rglob("*.rs"))
for relative in files:
    source = root / relative
    if source.is_file():
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
shutil.copytree(root / "test/round4_regressions", destination / "test/round4_regressions")
for name, relative in [
    ("patch", "crates/tool/src/patch.rs"),
    ("evolution", "crates/evolution/src/worker.rs"),
]:
    with (destination / relative).open("a", encoding="utf-8") as source:
        source.write(f'\n#[cfg(test)]\n#[path = "../../../test/round4_regressions/{name}.rs"]\nmod round4_regressions;\n')
with (destination / "crates/core/Cargo.toml").open("a", encoding="utf-8") as manifest:
    manifest.write('\n[dev-dependencies.sandbox]\npath = "../sandbox"\n\n[[test]]\nname = "round4_regressions_linux"\npath = "../../test/round4_regressions/linux.rs"\nharness = false\n')
for source in (destination / "crates").rglob("*.rs"):
    os.utime(source, None)
archive = destination.with_suffix(".tar")
with tarfile.open(archive, "w") as output:
    for source in destination.rglob("*"):
        if source.is_file():
            output.add(source, arcname=source.relative_to(destination))
print(destination)
print(archive)
