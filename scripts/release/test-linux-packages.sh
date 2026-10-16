#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
python3 - "$ROOT" "$TMP" <<'PY'
import importlib.util, json, os, sys, tarfile
from pathlib import Path
root, tmp = Path(sys.argv[1]), Path(sys.argv[2])
spec = importlib.util.spec_from_file_location("runtime", root / "scripts/release/linux-runtime.py")
runtime = importlib.util.module_from_spec(spec); sys.modules[spec.name] = runtime; spec.loader.exec_module(runtime)
for target, arch in (("x86_64-unknown-linux-gnu", "x86_64"), ("aarch64-unknown-linux-gnu", "aarch64")):
    version = "1.2.3"; pkgroot = tmp / f"lazydb_{version}_{target}"; pkgroot.mkdir()
    binary = pkgroot / "lazydb"; binary.write_bytes((target + "-binary").encode()); binary.chmod(0o755); archive = tmp / f"lazydb_{version}_{target}.tar.xz"
    with tarfile.open(archive, "w:xz") as tar: tar.add(pkgroot, arcname=pkgroot.name)
    elf = {"elf":{"machine":runtime.TARGETS[target]["machine"],"class":"ELF64","endianness":"little","interpreter":runtime.TARGETS[target]["interpreter"],"needed":["libc.so.6"],"rpath":[],"runpath":[]},"glibc":{"loader_min":"2.34","version_needs":[{"library":"libc.so.6","version":"GLIBC_2.34","weak":False}],"symbol_references":[]},"other_version_needs":[]}
    report = runtime.build_report(version,target,archive,binary,elf); report_path=tmp/f"{target}.runtime.json"; report_path.write_text(runtime.serialize_report(report))
    binary_report=dict(report); binary_report.pop("archive")
    bare_report=tmp/f"{target}.binary-report.json"; bare_report.write_text(json.dumps(binary_report))
    out=tmp/arch; out.mkdir()
    import subprocess
    fakebin=tmp/f"fake-{arch}"; fakebin.mkdir(); nfpm=fakebin/"nfpm"
    nfpm.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$PACKAGE_CALL_LOG"\nfor arg do if [ "$previous" = --target ]; then mkdir -p "$(dirname "$arg")"; : > "$arg"; fi; previous=$arg; done\n')
    nfpm.chmod(0o755); log=tmp/f"{arch}.calls"
    env=dict(os.environ,PATH=str(fakebin)+os.pathsep+os.environ["PATH"],PACKAGE_CALL_LOG=str(log))
    built=subprocess.run(["sh",str(root/"scripts/release/package-linux.sh"),version,arch,str(binary),str(report_path),str(out)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
    if built.returncode != 0: raise SystemExit("package rejected valid report: " + built.stderr.decode())
    calls=log.read_text()
    if len(calls.splitlines()) != 3: raise SystemExit("expected three package formats")
    generated=list(out.glob("*.deb"))+list(out.glob("*.rpm"))+list(out.glob("*.pkg.tar.zst"))
    if len(generated) != 3: raise SystemExit("expected all three package outputs after successful staging")
    failbin=tmp/f"fail-{arch}"; failbin.mkdir(); failing_nfpm=failbin/"nfpm"
    failing_nfpm.write_text('#!/bin/sh\ncase "$*" in *archlinux*) exit 1 ;; esac\nfor arg do if [ "$previous" = --target ]; then mkdir -p "$(dirname "$arg")"; : > "$arg"; fi; previous=$arg; done\n')
    failing_nfpm.chmod(0o755); partial_out=tmp/f"partial-{arch}"; partial_out.mkdir()
    failing_env=dict(os.environ, PATH=str(failbin)+os.pathsep+os.environ["PATH"])
    partial=subprocess.run(["sh",str(root/"scripts/release/package-linux.sh"),version,arch,str(binary),str(report_path),str(partial_out)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=failing_env)
    if partial.returncode == 0 or list(partial_out.iterdir()): raise SystemExit("failed package build left partial published packages")
    corrupted=tmp/f"{target}.corrupted"; corrupted.write_bytes(b"different binary"); corrupted.chmod(0o755)
    env_without_nfpm=dict(os.environ, PATH=os.pathsep.join(part for part in os.environ["PATH"].split(os.pathsep) if "fake-" not in part))
    result=subprocess.run(["sh",str(root/"scripts/release/package-linux.sh"),version,arch,str(corrupted),str(bare_report),str(out)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env_without_nfpm)
    if result.returncode == 0 or b"digest does not match" not in result.stderr: raise SystemExit("package accepted binary that differs from runtime report: " + result.stderr.decode() + result.stdout.decode())
print("Linux package preflight tests: ok")
PY
