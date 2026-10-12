function el<T extends HTMLElement=HTMLElement>(id:string):T {const e=document.getElementById(id);if(!e)throw Error(`Missing ${id}`);return e as T}
async function message(data:Record<string,unknown>):Promise<Record<string,unknown>>{const r=await chrome.runtime.sendMessage(data)as Record<string,unknown>;if(r.error)throw Error(String(r.error));return r}
function error(e:unknown):void{el('error').textContent=e instanceof Error?e.message:'操作失败'}
async function refresh():Promise<void>{try{const s=await message({type:'status'});el('state').textContent=s.state==='connected'?'已连接':'未就绪';el('state').className=s.state==='connected'?'pill online':'pill';el('detail').textContent=String(s.message??'');el('node').textContent=String(s.node??'本机 SerialBridge');if(document.activeElement!==el('port'))el<HTMLInputElement>('port').value=String(s.port??9527);el('tasks').textContent=String(s.activeTasks??0);}catch(e){error(e)}}
el('pair-form').addEventListener('submit',event=>{event.preventDefault();el('error').textContent='';void message({type:'configure',port:Number(el<HTMLInputElement>('port').value),code:el<HTMLInputElement>('code').value}).then(()=>{el<HTMLInputElement>('code').value='';void refresh()}).catch(error)});
el('disconnect').addEventListener('click',()=>void message({type:'disconnect'}).then(refresh).catch(error));
el('grant').addEventListener('click',()=>{
 try{const url=new URL(el<HTMLInputElement>('origin').value.trim());if(!['http:','https:'].includes(url.protocol)||url.username||url.password)throw Error('请输入 HTTP/HTTPS 网站地址');
 // Request immediately within the click gesture, not after a storage/network await.
 void chrome.permissions.request({origins:[`${url.origin}/*`]}).then(granted=>{el('permission').textContent=granted?`已授权 ${url.origin}。请同时在桌面设置中允许此来源。`:'未授予网站权限'}).catch(error);
 }catch(e){error(e)}
});
void refresh();setInterval(()=>void refresh(),1500);
