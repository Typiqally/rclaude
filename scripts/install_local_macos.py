#!/usr/bin/env python3
"""Install a local certificate-signed broker without reading Claude credentials.

The signing key remains in the login Keychain. This is local development signing,
not Developer ID signing or notarization. Authorization is a separate explicit CLI.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

NAME = "rclaude Local Development"
IDENTIFIER = "io.github.typiqally.rclaude"
ROOT = Path(__file__).resolve().parents[1]


def run(args, **kwargs):
    result = subprocess.run(args, check=True, capture_output=True, text=True,
                            timeout=kwargs.pop("timeout", 60), **kwargs)
    return result.stdout


def find_identity(output):
    for fingerprint, name in re.findall(r'\b([0-9A-Fa-f]{40})\s+"([^"]+)"', output):
        if name == NAME:
            return fingerprint.upper()
    return None


def requirement(fingerprint, identifier=IDENTIFIER):
    if not re.fullmatch(r"[0-9A-Fa-f]{40}", fingerprint):
        raise ValueError("Invalid public signing certificate fingerprint")
    if identifier not in (IDENTIFIER, IDENTIFIER + ".engine"):
        raise ValueError("Invalid broker signing identifier")
    return f'=designated => identifier "{identifier}" and certificate leaf = H"{fingerprint.upper()}"'


def launcher_reusable(destination, fingerprint, execute=run):
    if not destination.is_file() or destination.is_symlink():
        return False
    try:
        execute(["/usr/bin/codesign", "--verify", "--strict", "--test-requirement",
                 requirement(fingerprint).replace("=designated => ", "="), str(destination)])
        output = execute([str(destination), "--launcher-info"])
        if len(output) > 1024:
            return False
        return json.loads(output) == {"launcherVersion": 1, "engineAbiVersion": 1,
                                      "certificateSha1": fingerprint.upper()}
    except (ValueError, OSError, RuntimeError, subprocess.SubprocessError):
        return False


def install_pair(launcher_source, engine_source, destination, fingerprint, execute=run):
    if launcher_source is None and not launcher_reusable(destination, fingerprint, execute):
        raise RuntimeError("The installed launcher is unavailable or incompatible")
    destination.parent.mkdir(parents=True, exist_ok=True)
    engine_directory = destination.parent / ".rclaude"
    if engine_directory.is_symlink():
        raise RuntimeError("The engine directory must not be a symlink")
    engine_directory.mkdir(mode=0o700, exist_ok=True)
    engine_directory.chmod(0o700)
    staged = []
    def sign(source, directory, prefix, identifier, mode):
        descriptor, name = tempfile.mkstemp(prefix=prefix, dir=directory)
        os.close(descriptor)
        path = Path(name); staged.append(path)
        shutil.copyfile(source, path); path.chmod(mode)
        execute(["/usr/bin/codesign", "--force", "--sign", fingerprint, "--timestamp=none",
                 "--identifier", identifier, "--requirements", requirement(fingerprint, identifier),
                 str(path)], timeout=600)
        execute(["/usr/bin/codesign", "--verify", "--strict", "--test-requirement",
                 requirement(fingerprint, identifier).replace("=designated => ", "="), str(path)])
        return path
    try:
        engine = sign(engine_source, engine_directory, ".rclaude-engine-", IDENTIFIER + ".engine", 0o600)
        launcher = None
        if launcher_source is not None:
            launcher = sign(launcher_source, destination.parent, ".rclaude-launcher-", IDENTIFIER, 0o755)
            if not launcher_reusable(launcher, fingerprint, execute):
                raise RuntimeError("The staged launcher has the wrong identity or ABI")
        # Verify the complete candidate before replacing either installed file.
        if launcher is not None and destination.is_file():
            shutil.copyfile(destination, engine_directory / "previous-rclaude")
            (engine_directory / "previous-rclaude").chmod(0o700)
        os.replace(engine, engine_directory / "engine.dylib")
        if launcher is not None:
            os.replace(launcher, destination)
    finally:
        for path in staged:
            path.unlink(missing_ok=True)


def ensure_identity(execute=run):
    existing = find_identity(execute(["/usr/bin/security", "find-identity", "-p", "codesigning"]))
    if existing:
        return existing
    openssl = "/usr/bin/openssl"
    if not Path(openssl).is_file():
        raise RuntimeError("OpenSSL is required for local development signing")
    keychain = execute(["/usr/bin/security", "default-keychain", "-d", "user"]).strip().strip('"')
    if not keychain or not Path(keychain).is_file():
        raise RuntimeError("An existing login Keychain is required")
    # All temporary key files are private and removed immediately after import.
    # Import trusts only codesign to use this new signing key. No existing item,
    # certificate trust setting, partition list, or Claude ACL is modified here.
    previous_umask = os.umask(0o077)
    try:
        with tempfile.TemporaryDirectory(prefix="rclaude-local-signing-") as directory:
            private = Path(directory) / "private.pem"
            certificate = Path(directory) / "certificate.pem"
            traditional = Path(directory) / "rsa.pem"
            combined = Path(directory) / "identity.pem"
            execute([openssl, "req", "-new", "-newkey", "rsa:3072", "-nodes", "-x509", "-sha256",
                     "-days", "3650", "-subj", f"/CN={NAME}",
                     "-addext", "extendedKeyUsage=codeSigning", "-addext", "keyUsage=digitalSignature",
                     "-addext", "basicConstraints=critical,CA:FALSE",
                     "-keyout", str(private), "-out", str(certificate)])
            # Apple's OpenSSL import format expects PKCS#1, not an unencrypted
            # PKCS#8 PRIVATE KEY wrapper. Convert before importing the PEM sequence.
            execute([openssl, "rsa", "-in", str(private), "-out", str(traditional)])
            combined.write_bytes(traditional.read_bytes() + certificate.read_bytes())
            execute(["/usr/bin/security", "import", str(combined), "-k", keychain,
                     "-f", "pemseq", "-t", "agg", "-x", "-T", "/usr/bin/codesign"])

    finally:
        os.umask(previous_umask)
    created = find_identity(execute(["/usr/bin/security", "find-identity", "-p", "codesigning"]))
    if not created:
        raise RuntimeError("The local signing identity is unavailable; no authorization was attempted")
    return created


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=Path("/opt/homebrew/bin/rclaude"))
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise RuntimeError("This installer requires macOS")
    fingerprint = ensure_identity()
    destination = args.destination.absolute()
    reuse = launcher_reusable(destination, fingerprint)
    if not args.skip_build:
        environment = os.environ.copy()
        environment["RCLAUDE_SIGNING_CERTIFICATE_SHA1"] = fingerprint
        command = ["cargo", "build", "--release", "--locked", "--lib"]
        if not reuse:
            command += ["--bin", "rclaude-launcher"]
        run(command, cwd=ROOT, env=environment, timeout=600)
    engine = ROOT / "target/release/librclaude.dylib"
    launcher = None if reuse else ROOT / "target/release/rclaude-launcher"
    if not engine.is_file() or (launcher is not None and not launcher.is_file()):
        raise RuntimeError("The release engine or launcher is unavailable")
    install_pair(launcher, engine, destination, fingerprint)
    print(f"Installed signed in-process broker engine: {destination}")
    print("Normal broker requests never authorize interactively.")
    if reuse:
        print("The authorized launcher bytes were preserved.")
    else:
        print(f"Authorize this launcher explicitly once: {destination} authorize")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, subprocess.SubprocessError) as error:
        # Never print raw subprocess stderr or any private key material.
        print(f"Local broker installation failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
