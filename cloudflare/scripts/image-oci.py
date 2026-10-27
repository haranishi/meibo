"""Export a local Docker image as an OCI layout; never confuse config/manifest digests.

Input is a JSON array of secret values on stdin for an exact byte scan, not argv.
Only the caller's freshly built image is accepted. No archive paths are extracted.
"""
import hashlib
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


def main():
    image, destination = sys.argv[1:]
    secrets = [value.encode() for value in json.load(sys.stdin)]
    target = Path(destination)
    target.mkdir(mode=0o700)
    blobs = target / "blobs" / "sha256"
    blobs.mkdir(mode=0o700, parents=True)

    def blob(data, media_type):
        digest = hashlib.sha256(data).hexdigest()
        path = blobs / digest
        with path.open("xb") as output:
            os.chmod(path, 0o600)
            output.write(data)
        return {"mediaType": media_type, "digest": "sha256:" + digest, "size": len(data)}

    def encoded(value):
        return json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode()

    with tempfile.TemporaryFile(dir=target.parent) as saved:
        subprocess.run([os.environ.get("MEIBO_DOCKER", "docker"), "image", "save", image], stdout=saved, stderr=subprocess.PIPE, check=True)
        saved.seek(0)
        with tarfile.open(fileobj=saved, mode="r:") as archive:
            entries = json.load(archive.extractfile("manifest.json"))
            if len(entries) != 1:
                raise ValueError("ambiguous image")
            entry = entries[0]

            def read_member(name):
                member = archive.getmember(name)
                if not member.isfile() or member.size > 256 * 1024 * 1024:
                    raise ValueError("invalid image member")
                data = archive.extractfile(member).read()
                if any(secret in data for secret in secrets):
                    raise ValueError("credential in image")
                return data

            config_bytes = read_member(entry["Config"])
            config = json.loads(config_bytes)
            config_descriptor = blob(config_bytes, "application/vnd.oci.image.config.v1+json")
            layers = []
            for name in entry["Layers"]:
                data = read_member(name)
                # Docker's containerd image store can retain compressed OCI
                # blobs in docker save; rootfs.diff_ids always hashes raw tar.
                if data.startswith(b"\x1f\x8b"):
                    data = gzip.decompress(data)
                if len(data) > 256 * 1024 * 1024 or any(secret in data for secret in secrets):
                    raise ValueError("invalid image layer")
                layers.append(blob(data, "application/vnd.oci.image.layer.v1.tar"))
            if [layer["digest"] for layer in layers] != config["rootfs"]["diff_ids"]:
                raise ValueError("image layer mismatch")
            manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json", "config": config_descriptor, "layers": layers}
            descriptor = blob(encoded(manifest), manifest["mediaType"])
    for name, value in [("oci-layout", {"imageLayoutVersion": "1.0.0"}),
                        ("index.json", {"schemaVersion": 2, "manifests": [descriptor]})]:
        with (target / name).open("xb") as output:
            os.chmod(target / name, 0o600)
            output.write(encoded(value))
    print(json.dumps({"digest": descriptor["digest"], "configDigest": config_descriptor["digest"], "layerCount": len(layers), "credentialsInImage": False}))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("Image export/verification failed; private diagnostics suppressed.", file=sys.stderr)
        sys.exit(1)
