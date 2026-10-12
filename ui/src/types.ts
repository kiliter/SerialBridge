import type {LogEntry} from './model.js';
export interface Snapshot {version:string;node:{name:string;os:string};http:{port:number};serial:{port:string;baud_rate:number};peer:{connected:boolean;name?:string;os?:string;browser?:boolean};browser:{connected:boolean;origin?:string};pending_requests:number}
export interface Transfer {id:string;filename:string;direction:string;target:string;status:string;stage:string;received_bytes:number;total_bytes:number|null;sha256?:string;error?:string;created_at:number}
export interface Settings {node_name:string;serial_port:string;baud_rate:number;http_port:number;allowed_origins:string[];allowed_methods:string[];auto_reconnect:boolean;download_dir:string;shared_dir:string;max_file_bytes:number;max_spool_bytes:number;chunk_bytes:number;extension_origin?:string}
export interface Port {name:string;product?:string;serial_number?:string}
export interface State {snapshot:Snapshot|null;logs:LogEntry[];transfers:Transfer[];settings:Settings|null;ports:Port[];page:string;paused:boolean;filter:{query:string;level:string;component:string};error:string}
declare global {interface Window {__TAURI__?:{core:{invoke<T>(command:string,args?:Record<string,unknown>):Promise<T>}}}}
