#!/usr/bin/env python3
"""Build the Oink Android app.

1. liboink_android.so for each ABI (cargo + Android NDK),
2. the shared interface (apps/oink) into app/src/main/assets/web,
3. the APK with Gradle.

Requires ANDROID_HOME (SDK with platform 35 and build-tools) and the NDK
version named in app/build.gradle.kts, Node/npm for the interface, and the
Rust targets (`rustup target add aarch64-linux-android x86_64-linux-android`).

    python apps/android/build.py                 # debug-signed APK, arm64-v8a
    python apps/android/build.py --abi arm64-v8a --abi x86_64
"""
import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
APP = HERE / "app"
MIN_SDK = 26
ABIS = {
    "arm64-v8a": "aarch64-linux-android",
    "x86_64": "x86_64-linux-android",
}


def run(args, **kwargs):
    print("+", " ".join(str(a) for a in args), flush=True)
    subprocess.run([str(a) for a in args], check=True, **kwargs)


def ndk_dir() -> Path:
    version = re.search(r'ndkVersion = "([^"]+)"', (APP / "build.gradle.kts").read_text()).group(1)
    for var in ("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"):
        if os.environ.get(var):
            return Path(os.environ[var])
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        sys.exit("ANDROID_HOME is not set")
    path = Path(sdk) / "ndk" / version
    if not path.is_dir():
        sys.exit(f"NDK {version} not found at {path}; install it with sdkmanager \"ndk;{version}\"")
    return path


def toolchain(ndk: Path) -> Path:
    host = {"Linux": "linux-x86_64", "Darwin": "darwin-x86_64", "Windows": "windows-x86_64"}[platform.system()]
    return ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin"


def build_rust(abis, release: bool):
    bin_dir = toolchain(ndk_dir())
    suffix = ".cmd" if platform.system() == "Windows" else ""
    exe = ".exe" if platform.system() == "Windows" else ""
    for abi in abis:
        target = ABIS[abi]
        env = dict(os.environ)
        clang = bin_dir / f"{target}{MIN_SDK}-clang{suffix}"
        key = target.replace("-", "_")
        env[f"CC_{key}"] = str(clang)
        env[f"CXX_{key}"] = str(bin_dir / f"{target}{MIN_SDK}-clang++{suffix}")
        env[f"AR_{key}"] = str(bin_dir / f"llvm-ar{exe}")
        env[f"CARGO_TARGET_{key.upper()}_LINKER"] = str(clang)
        profile = ["--release"] if release else []
        run(["cargo", "build", "-p", "oink-android", "--target", target, *profile], cwd=ROOT, env=env)
        built = ROOT / "target" / target / ("release" if release else "debug") / "liboink_android.so"
        out = APP / "src" / "main" / "jniLibs" / abi
        out.mkdir(parents=True, exist_ok=True)
        # Strip debug info: the unstripped library is several times larger.
        run([bin_dir / f"llvm-strip{exe}", "--strip-debug", "-o", out / "liboink_android.so", built])


def build_web(skip_npm: bool):
    npm = "npm.cmd" if platform.system() == "Windows" else "npm"
    if not skip_npm:
        run([npm, "run", "-w", "apps/oink", "build"], cwd=ROOT)
    dist = ROOT / "apps" / "oink" / "dist"
    out = APP / "src" / "main" / "assets" / "web"
    shutil.rmtree(out, ignore_errors=True)
    shutil.copytree(dist, out)
    shutil.copy(HERE / "web" / "oink-android.js", out / "oink-android.js")
    index = out / "index.html"
    html = index.read_text(encoding="utf-8")
    # The bridge must exist before the interface's module scripts run, and
    # after the viewport meta it adjusts on narrow screens.
    tag = '<meta name="viewport"'
    end = html.index(">", html.index(tag)) + 1
    html = html[:end] + '\n    <script src="/oink-android.js"></script>' + html[end:]
    index.write_text(html, encoding="utf-8")


def build_apk(release: bool):
    gradle = os.environ.get("GRADLE", "gradle")
    run([gradle, "-p", HERE, "--no-daemon", "assembleRelease" if release else "assembleDebug"])
    kind = "release" if release else "debug"
    for apk in sorted((APP / "build" / "outputs" / "apk" / kind).glob("*.apk")):
        print(apk)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--release", action="store_true", help="a release APK (needs your signing config)")
    parser.add_argument("--rust-debug", action="store_true", help="unoptimized Rust (large and slow)")
    parser.add_argument("--abi", action="append", choices=sorted(ABIS), help="default: arm64-v8a")
    parser.add_argument("--skip-rust", action="store_true")
    parser.add_argument("--skip-web", action="store_true")
    parser.add_argument("--skip-npm", action="store_true", help="reuse an existing apps/oink/dist")
    parser.add_argument("--skip-apk", action="store_true")
    args = parser.parse_args()
    if not args.skip_rust:
        build_rust(args.abi or ["arm64-v8a"], not args.rust_debug)
    if not args.skip_web:
        build_web(args.skip_npm)
    if not args.skip_apk:
        build_apk(args.release)


if __name__ == "__main__":
    main()
