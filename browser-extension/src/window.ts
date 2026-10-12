/** At most one unacknowledged binary block per download. */
export class AckWindow {
 private pending=new Map<string,{offset:number;resolve:()=>void;reject:(e:Error)=>void;clear:()=>void}>();
 wait(id:string,offset:number,signal:AbortSignal,timeout=15000):Promise<void>{
  if(this.pending.has(id))return Promise.reject(Error('WINDOW_FULL'));
  signal.throwIfAborted();
  return new Promise((resolve,reject)=>{
   const abort=()=>this.reject(id,Error('CANCELLED'));
   const timer=setTimeout(()=>this.reject(id,Error('ACK_TIMEOUT')),timeout);
   const clear=()=>{clearTimeout(timer);signal.removeEventListener('abort',abort)};
   this.pending.set(id,{offset,resolve,reject,clear});signal.addEventListener('abort',abort,{once:true});
  });
 }
 ack(id:string,offset:number):boolean{
  const p=this.pending.get(id);if(!p)return false;
  if(p.offset!==offset){this.reject(id,Error('ACK_OFFSET_MISMATCH'));return false}
  this.pending.delete(id);p.clear();p.resolve();return true;
 }
 reject(id:string,error:Error):void{const p=this.pending.get(id);if(p){this.pending.delete(id);p.clear();p.reject(error)}}
 reset():void{for(const id of this.pending.keys())this.reject(id,Error('BROWSER_DISCONNECTED'))}
}
