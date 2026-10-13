/** Test-only CommonJS bundle for an about:blank DOM. It opens no blocked URL. */
import fs from 'node:fs';import path from 'node:path';import {createRequire} from 'node:module';import {execFileSync} from 'node:child_process';
const require=createRequire(import.meta.url);let ts;try{ts=require('typescript')}catch{const globalRoot=execFileSync(process.platform==='win32'?'npm.cmd':'npm',['root','-g'],{encoding:'utf8'}).trim();ts=require(path.join(globalRoot,'typescript'))}
const modules=['model','types','api','main'];let out='(()=>{const modules={};const cache={};';
for(const name of modules){const js=fs.readFileSync(`ui/dist/assets/${name}.js`,'utf8');const compiled=ts.transpileModule(js,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS}}).outputText;out+=`modules['${name}']=function(module,exports,require){\n${compiled}\n};\n`;}
out+=`function require(id){id=id.replace('./','').replace('.js','');if(cache[id])return cache[id].exports;const module={exports:{}};cache[id]=module;modules[id](module,module.exports,require);return module.exports;}require('main');})();`;
const output=process.env.SERIALBRIDGE_UI_TEST_OUTPUT??'.superpowers/ui-qa';
fs.mkdirSync(output,{recursive:true});
fs.writeFileSync(path.join(output,'offline-ui-test-bundle.js'),out);
