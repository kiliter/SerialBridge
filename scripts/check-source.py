"""Packaging/configuration sanity checks; NOT a Rust compiler or runtime test."""
from pathlib import Path
import json,tomllib,re
ROOT=Path(__file__).resolve().parents[1]
for p in ROOT.rglob('*.json'):
    json.loads(p.read_text())
for p in ROOT.rglob('*.toml'):
    tomllib.loads(p.read_text())
for module in ['api','config','error','extension','http_executor','node','protocol','transfer','transport']:
    assert (ROOT/f'crates/core/src/{module}.rs').is_file(),module
for p in ['ui/dist/index.html','ui/dist/assets/main.js','browser-extension/dist/manifest.json','browser-extension/dist/background.js','crates/desktop/tauri.conf.json']:
    assert (ROOT/p).is_file(),p
for p in ROOT.rglob('*.rs'):
    s=p.read_text();assert 'todo!(' not in s and 'unimplemented!(' not in s,p
rust_tests=sum(len(re.findall(r'#\[(?:tokio::)?test\]',p.read_text()))for p in ROOT.rglob('*.rs'))
print(f'PASS: JSON/TOML parse, expected module/build assets exist, no todo!/unimplemented! macros. {rust_tests} Rust test functions are present but have NOT been executed.')
