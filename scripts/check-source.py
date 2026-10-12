"""Packaging/configuration sanity checks; NOT a Rust compiler or runtime test."""
from pathlib import Path
import json,tomllib,re,os
ROOT=Path(__file__).resolve().parents[1]
files=[]
for directory,dirs,names in os.walk(ROOT):
    dirs[:]=[d for d in dirs if d not in {'target','node_modules','.git','.superpowers','artifacts','__pycache__'}]
    files.extend(Path(directory)/name for name in names)
for p in (p for p in files if p.suffix=='.json'):
    json.loads(p.read_text())
for p in (p for p in files if p.suffix=='.toml'):
    tomllib.loads(p.read_text())
for module in ['api','config','error','extension','http_executor','node','protocol','transfer','transport']:
    assert (ROOT/f'crates/core/src/{module}.rs').is_file(),module
for p in ['ui/dist/index.html','ui/dist/assets/main.js','browser-extension/dist/manifest.json','browser-extension/dist/background.js','crates/desktop/tauri.conf.json']:
    assert (ROOT/p).is_file(),p
for p in (p for p in files if p.suffix=='.rs'):
    s=p.read_text();assert 'todo!(' not in s and 'unimplemented!(' not in s,p
rust_tests=sum(len(re.findall(r'#\[(?:tokio::)?test\]',p.read_text()))for p in files if p.suffix=='.rs')
print(f'PASS: source JSON/TOML parse, expected module/build assets exist, no todo!/unimplemented! macros. {rust_tests} Rust test functions detected; execution results come from cargo test logs, not this static check.')
