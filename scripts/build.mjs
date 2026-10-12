import {spawnSync} from 'node:child_process';
import {cpSync,mkdirSync} from 'node:fs';
for(const part of ['ui','browser-extension']) {
 const command=process.platform==='win32'?'tsc.cmd':'tsc';
 const result=spawnSync(command,['-p',`${part}/tsconfig.json`],{stdio:'inherit',shell:process.platform==='win32'});
 if(result.error) {console.error('Install dev dependencies with npm install, or make tsc available in PATH.',result.error.message);process.exit(1)}
 if(result.status!==0) process.exit(result.status??1);
 mkdirSync(`${part}/dist`,{recursive:true});
 cpSync(`${part}/public`,`${part}/dist`,{recursive:true});
}
console.log('Built console and browser extension. This does not compile Rust/Tauri.');
