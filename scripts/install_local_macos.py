#!/usr/bin/env python3
"""Install a local certificate-signed broker without reading Claude credentials.

The signing key remains in the login Keychain. This is local development signing,
not Developer ID signing or notarization. Authorization is a separate explicit CLI.
"""
import argparse
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


def requirement(fingerprint):
    if not re.fullmatch(r"[0-9A-Fa-f]{40}", fingerprint):
        raise ValueError("Invalid public signing certificate fingerprint")
    return f'=designated => identifier "{IDENTIFIER}" and certificate leaf = H"{fingerprint.upper()}"'


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
    if not args.skip_build:
        run(["cargo", "build", "--release", "--locked"], cwd=ROOT, timeout=600)
    source = ROOT / "target/release/rclaude"
    if not source.is_file():
        raise RuntimeError("The release binary is unavailable")
    fingerprint = ensure_identity()
    destination = args.destination.absolute()
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Install atomically and preserve the current binary until signing verifies.
    descriptor, temporary = tempfile.mkstemp(prefix=".rclaude-install-", dir=destination.parent)
    os.close(descriptor)
    staged = Path(temporary)
    try:
        shutil.copyfile(source, staged)
        staged.chmod(0o755)
        print("macOS may request use of the local signing key. Approve in the system dialog.", flush=True)
        run(["/usr/bin/codesign", "--force", "--sign", fingerprint, "--timestamp=none",
             "--identifier", IDENTIFIER, "--requirements", requirement(fingerprint), str(staged)], timeout=600)
        run(["/usr/bin/codesign", "--verify", "--strict", str(staged)])
        os.replace(staged, destination)
        run(["/usr/bin/codesign", "--verify", "--strict", str(destination)])
    finally:
        staged.unlink(missing_ok=True)
    print(f"Installed stable certificate-signed broker: {destination}")
    print("Normal broker requests never authorize interactively.")
    print(f"Explicit authorization command: {destination} authorize")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, subprocess.SubprocessError) as error:
        # Never print raw subprocess stderr or any private key material.
        print(f"Local broker installation failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
