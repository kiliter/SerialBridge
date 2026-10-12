"""Build an unpacked-extension ZIP with manifest.json at archive root."""
import json
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def package(source=ROOT / "browser-extension/dist", output_dir=ROOT / "artifacts"):
    manifest = json.loads((source / "manifest.json").read_text(encoding="utf-8"))
    for name in (manifest["background"]["service_worker"], manifest["action"]["default_popup"]):
        if not (source / name).is_file():
            raise FileNotFoundError(f"Build the extension first: missing {name}")
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / f'SerialBridge-Chrome-{manifest["version_name"]}.zip'
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(path.relative_to(source).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                archive.writestr(info, path.read_bytes())
    return output


if __name__ == "__main__":
    print(package())
