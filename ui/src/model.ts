export interface LogEntry {time?:number;level:string;component:string;message:string;id?:string}
export function deriveStatus(snapshot:unknown):{core:boolean;peer:boolean;localBrowser:boolean;remoteBrowser:boolean} {
 if(!snapshot||typeof snapshot!=='object')return {core:false,peer:false,localBrowser:false,remoteBrowser:false};
 const s=snapshot as {peer?:{connected?:boolean;browser?:boolean};browser?:{connected?:boolean}};
 return {core:true,peer:s.peer?.connected===true,localBrowser:s.browser?.connected===true,remoteBrowser:s.peer?.connected===true&&s.peer?.browser===true};
}
export function filterLogs(logs:LogEntry[],f:{level:string;component:string;query:string}):LogEntry[]{
 return logs.filter(x=>(!f.level||f.level==='all'||x.level===f.level)&&(!f.component||f.component==='all'||x.component===f.component)&&`${x.message} ${x.id??''} ${x.component}`.toLowerCase().includes(f.query.toLowerCase()));
}
export function progress(received:number,total:number|null|undefined):number|null {return total&&total>0?Math.min(100,Math.max(0,100*received/total)):null}
export function escapeHtml(x:unknown):string {return String(x??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]!))}
