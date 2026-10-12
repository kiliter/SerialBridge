interface ChromeSender {id?:string;url?:string}
declare const chrome:{
 runtime:{id:string;getURL(path:string):string;onMessage:{addListener(cb:(message:unknown,sender:ChromeSender,respond:(value:unknown)=>void)=>boolean|void):void};onInstalled:{addListener(cb:()=>void):void};onStartup:{addListener(cb:()=>void):void};sendMessage(message:unknown):Promise<unknown>};
 storage:{local:{get(keys:string[]|string):Promise<Record<string,unknown>>;set(value:Record<string,unknown>):Promise<void>;remove(keys:string|string[]):Promise<void>;setAccessLevel?(value:{accessLevel:'TRUSTED_CONTEXTS'}):Promise<void>}};
 permissions:{contains(value:{origins:string[]}):Promise<boolean>;request(value:{origins:string[]}):Promise<boolean>;remove(value:{origins:string[]}):Promise<boolean>;getAll():Promise<{origins?:string[]}>};
 alarms:{create(name:string,info:{periodInMinutes:number}):Promise<void>;onAlarm:{addListener(cb:(alarm:{name:string})=>void):void}};
};
