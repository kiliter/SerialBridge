import type {LogEntry} from './model.js';
export interface Snapshot {version:string;node:{name:string;os:string};http:{port:number};serial:{port:string;baud_rate:number};peer:{connected:boolean;name?:string;os?:string;browser?:boolean};browser:{connected:boolean;origin?:string};pending_requests:number;tunnel?:{forwards:number;active_streams:number}}
export interface Transfer {id:string;filename:string;direction:string;target:string;status:string;stage:string;received_bytes:number;total_bytes:number|null;sha256?:string;error?:string;created_at:number}
export interface Settings {node_name:string;serial_port:string;baud_rate:number;http_port:number;allowed_origins:string[];allowed_methods:string[];auto_reconnect:boolean;download_dir:string;shared_dir:string;max_file_bytes:number;max_spool_bytes:number;chunk_bytes:number;extension_origin?:string;forwards:Forward[]}
export interface Forward {listen:string;target:string}
export interface ListenerStatus extends Forward {listening:boolean;error?:string|null}
export interface TunnelStream {id:string;direction:'outbound'|'inbound';listen?:string|null;target:string;client?:string|null;started_at:number;bytes_sent:number;bytes_received:number}
export interface TunnelSnapshot {forwards:ListenerStatus[];streams:TunnelStream[];total_streams:number}
export interface Port {name:string;product?:string;serial_number?:string}
export interface State {snapshot:Snapshot|null;logs:LogEntry[];transfers:Transfer[];tunnels:TunnelSnapshot|null;settings:Settings|null;ports:Port[];page:string;paused:boolean;filter:{query:string;level:string;component:string};error:string}
declare global {interface Window {__TAURI__?:{core:{invoke<T>(command:string,args?:Record<string,unknown>):Promise<T>}}}}
