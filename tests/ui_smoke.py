"""UI-only smoke test. Uses opt-in demo and intercepted HTTP fixtures, NOT Rust or hardware."""
from pathlib import Path
from functools import partial
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
import json, threading, time
from playwright.sync_api import sync_playwright, expect
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'docs/verification'
OUT.mkdir(exist_ok=True)
class Quiet(SimpleHTTPRequestHandler):
    def log_message(self,*args): pass
server=ThreadingHTTPServer(('127.0.0.1',4173),partial(Quiet,directory=str(ROOT/'ui/dist')))
threading.Thread(target=server.serve_forever,daemon=True).start()
results=[]
def passed(name): results.append(name); print('PASS',name,flush=True)
try:
 with sync_playwright() as p:
  browser=p.chromium.launch(executable_path='/usr/bin/chromium',headless=True,args=['--no-sandbox'])
  page=browser.new_page(viewport={'width':1280,'height':900},device_scale_factor=1)
  errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
  page.goto('http://127.0.0.1:4173/?demo=1')
  expect(page.locator('#demo-banner')).to_be_visible()
  expect(page.locator('#side-name')).to_have_text('Node A')
  page.screenshot(path=str(OUT/'ui-demo-overview.png'),full_page=True)
  passed('overview has explicit demo label and distinct local/peer browser cards')
  page.locator('[data-page="logs"]').click()
  expect(page.locator('#log-rows tr')).to_have_count(4)
  page.locator('#log-query').fill('ORIGIN_NOT_ALLOWED')
  expect(page.locator('#log-rows tr')).to_have_count(1)
  page.locator('#log-rows tr').first.click()
  expect(page.locator('#dialog')).to_be_visible()
  expect(page.locator('#dialog-content')).to_contain_text('ORIGIN_NOT_ALLOWED')
  page.locator('#close-dialog').click()
  page.locator('#log-query').fill('')
  page.locator('#pause-logs').click()
  expect(page.locator('#pause-logs')).to_have_text('继续接收')
  page.screenshot(path=str(OUT/'ui-demo-logs.png'),full_page=True)
  passed('logs support search, details and pausing')
  with page.expect_download() as download:
   page.locator('#export-logs').click()
  assert download.value.suggested_filename=='serialbridge-logs.jsonl'
  passed('browser console exports filtered logs')
  page.locator('[data-page="transfers"]').click()
  page.locator('#new-download').click()
  page.locator('#download-form input[name="url"]').fill('https://example.com/file.zip')
  page.locator('#download-form input[name="filename"]').fill('smoke.zip')
  page.locator('#download-form button').click()
  expect(page.locator('#dialog')).not_to_be_visible()
  row=page.locator('.transfer-live').filter(has_text='smoke.zip')
  expect(row).to_be_visible()
  row.locator('[data-cancel]').click()
  expect(row).to_contain_text('已取消')
  page.screenshot(path=str(OUT/'ui-demo-transfers.png'),full_page=True)
  passed('download form and cancellation update transfer cards')
  page.locator('[data-page="settings"]').click()
  expect(page.locator('#settings-form')).to_be_visible()
  page.locator('#settings-form input[name="node_name"]').fill('UI Test Node')
  page.locator('#settings-form button[type="submit"]').click()
  expect(page.locator('#toast')).to_contain_text('设置已保存')
  page.locator('#new-pair').click()
  expect(page.locator('.pair-code')).to_have_text('DEMO00000000')
  page.screenshot(path=str(OUT/'ui-demo-settings.png'),full_page=True)
  passed('settings save and short-lived pairing code rendering')
  assert not errors,errors
  passed('all four demo pages have zero uncaught JavaScript errors')
  live=browser.new_page(viewport={'width':1280,'height':900})
  live.add_init_script("sessionStorage.setItem('serialbridge.token','TEST-ONLY-TOKEN');")
  fixture={'offline':False,'peer':True}
  snapshot={'version':'test','node':{'name':'Fixture A','os':'test'},'http':{'port':9527},'serial':{'port':'TEST','baud_rate':115200},'peer':{'connected':True,'browser':True,'name':'Fixture B','os':'test'},'browser':{'connected':True},'pending_requests':0}
  def route(r):
   if fixture['offline']:r.abort();return
   path=r.request.url.split(':9527')[-1]
   if path=='/v1/health':
    snapshot['peer']['connected']=fixture['peer'];data=snapshot
   elif path=='/v1/logs':data={'logs':[{'time':1,'level':'error','component':'browser','message':'<img id="injected" src=x onerror="alert(1)">','id':'fixture-xss'}]}
   elif path=='/v1/downloads':data={'transfers':[]}
   else:data={}
   r.fulfill(status=200,content_type='application/json',headers={'access-control-allow-origin':'http://127.0.0.1:4173','access-control-allow-headers':'Authorization,Content-Type'},body=json.dumps(data))
  live.route('http://127.0.0.1:9527/**',route)
  live.goto('http://127.0.0.1:4173/')
  expect(live.locator('#side-name')).to_have_text('Fixture A')
  expect(live.locator('#demo-banner')).not_to_be_visible()
  expect(live.locator('#injected')).to_have_count(0)
  passed('live mode renders escaped API fixture data, not automatic demo data')
  fixture['peer']=False
  live.locator('#refresh').click()
  expect(live.locator('.stat-value').nth(3)).to_have_text('状态未知')
  passed('peer disconnection invalidates remote browser online status')
  fixture['offline']=True
  live.locator('#refresh').click()
  expect(live.locator('#connection-pill')).to_have_text('本机服务未连接')
  expect(live.locator('#error-banner')).to_be_visible()
  expect(live.locator('#demo-banner')).not_to_be_visible()
  passed('API failure reports offline and never falls back to demo mode')
  browser.close()
finally:
 server.shutdown()
 (OUT/'ui-smoke-results.json').write_text(json.dumps({'scope':'UI only; demo data and intercepted HTTP fixtures, not Rust/hardware','passed':results,'count':len(results)},ensure_ascii=False,indent=2))
print(f'{len(results)} UI checks passed')
