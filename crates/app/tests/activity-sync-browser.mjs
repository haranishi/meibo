// Exercise the bundled worker and both real CRM entry points without contacting an OAuth provider.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join,resolve } from "node:path";
import { createInterface } from "node:readline";

const playwright=import.meta.resolve(process.env.PLAYWRIGHT_MODULE || "playwright");
const {chromium}=await import(playwright);
const {expect}=createRequire(playwright)("playwright/test");
assert.ok(process.env.MEIBO_BINARY);
const binary=resolve(process.env.MEIBO_BINARY);
const scratch=await mkdtemp(join(tmpdir(),"meibo-activity-browser-"));
let child,browser;
function syntheticFetch(awayMarker, syntheticTime) {
  const fs=require("node:fs");
  global.fetch=async(input,options)=>{
    const u=new URL(input);
    const google=u.hostname.endsWith("googleapis.com");
    const now=syntheticTime, start=new Date(now).toISOString(), end=new Date(now+3600000).toISOString();
    const address="buyer@example.invalid";
    const revision=fs.existsSync(awayMarker)?Number(fs.readFileSync(awayMarker,"utf8")):0;
    let data;
    if(u.pathname.endsWith("/token")) data={access_token:"synthetic-access",refresh_token:"synthetic-refresh",expires_in:3600};
    else if(u.pathname.endsWith("/profile")) data={emailAddress:"owner@example.invalid",historyId:"initial"};
    else if(u.pathname.endsWith("/userinfo")) data={sub:"synthetic-subject",email:"owner@example.invalid"};
    else if(u.pathname.endsWith("/history")) data={historyId:"latest",history:u.searchParams.get("startHistoryId")==="initial"?[{messagesAdded:[{message:{id:"inbox-mail"}}]}]:[]};
    else if(google && u.pathname.includes("/messages/")) data={id:"inbox-mail",internalDate:String(now),payload:{headers:[
      {name:"From",value:address},{name:"Subject",value:"Google fixture mail"}]}};
    else if(google && u.pathname.endsWith("/events")) {
      const event={id:"g-event",summary:"Google fixture event",start:{dateTime:start},end:{dateTime:end},attendees:[{email:address}]};
      const changed={...event,id:"after-close",summary:"Closed-tab fixture event "+revision};
      const token=u.searchParams.get("syncToken");
      if (revision && token===null) fs.appendFileSync(awayMarker+".full","google\n");
      const baseline=revision?[event,changed]:[event];
      const items=token===null ? baseline : token===String(revision) ? [] : [changed];
      data={items,nextSyncToken:String(revision)};
    }
    else if(u.hostname==="graph.microsoft.com" && u.pathname.includes("/mailFolders/")) data={
      value:u.pathname.includes("/inbox/")&&!u.searchParams.has("$deltatoken")?[{id:"ms-mail",subject:"Outlook fixture mail",receivedDateTime:start,from:{emailAddress:{address}}}]:[],
      "@odata.deltaLink":u.origin+u.pathname+"?$deltatoken=next"};
    else if(u.hostname==="graph.microsoft.com" && u.pathname.endsWith("/calendarView/delta")) {
      const token=u.searchParams.get("$deltatoken");
      if (revision && token===null) fs.appendFileSync(awayMarker+".full","microsoft\n");
      const event={id:"ms-event",subject:revision?"Outlook fixture event "+revision:"Outlook fixture event",
        start:{dateTime:start,timeZone:"UTC"},end:{dateTime:end,timeZone:"UTC"},attendees:[{emailAddress:{address}}]};
      data={value:token===String(revision)?[]:[event],"@odata.deltaLink":u.origin+u.pathname+"?$deltatoken="+revision};
    }
    else throw new Error("Unexpected synthetic request");
    assertSyntheticRequest(options);return Response.json(data);
  };
  function assertSyntheticRequest(options) {if(options.redirect!=="error") throw new Error("Unexpected redirect policy");}
}
try {
  const probe=createServer();await new Promise(resolvePort=>probe.listen(0,"127.0.0.1",resolvePort));
  const port=probe.address().port;await new Promise(r=>probe.close(r));
  const origin="http://127.0.0.1:"+port;
  const db=join(scratch,"only-invented-records.db"),vault=join(scratch,"credentials");
  const mock=join(scratch,"mock.cjs"),node=join(scratch,"node-fixture");
  await mkdir(vault,{mode:0o700});
  const marker=join(scratch,"while-tab-was-closed");
  await writeFile(marker+".full","");
  await writeFile(mock,"("+syntheticFetch.toString()+")("+JSON.stringify(marker)+","+String(Date.now()+30000)+");");
  const quote=s=>"'"+s.replaceAll("'","'\\''")+"'";
  await writeFile(node,"#!/bin/sh\nexec "+quote(process.execPath)+" --require "+quote(mock)+' "$@"\n',{mode:0o700});
  assert.equal(spawnSync(binary,["init",db,"--origin","real"]).status,0);
  const env={...process.env,MEIBO_SYNC_VAULT:vault,MEIBO_ACTIVITY_SYNC_NODE:node,MEIBO_SYNC_ORIGIN:origin};
  for(const p of ["GOOGLE","MICROSOFT"]) {
    env["MEIBO_SYNC_"+p+"_CLIENT_ID"]="synthetic";env["MEIBO_SYNC_"+p+"_CLIENT_SECRET"]="fixture-only";
  }
  child=spawn(binary,["serve",db,"--allow-real","--port",String(port)],{env,stdio:["ignore","pipe","ignore"]});
  await new Promise((r,j)=>{
    const timer=setTimeout(()=>j(Error("fixture did not start")),10000);
    createInterface({input:child.stdout}).once("line",line=>{clearTimeout(timer);assert.equal(line,origin);r();});
    child.once("exit",()=>{clearTimeout(timer);j(Error("fixture exited"));});
  });
  const post=async(path,body)=>{
    const res=await fetch(origin+path,{method:"POST",headers:{Origin:origin,"X-Meibo-Local":"1","Content-Type":"application/json"},body:JSON.stringify(body)});
    assert.ok(res.ok,path+" "+res.status);return res.json();
  };
  const company=await post("/api/companies",{name:"架空の連携企業",prefecture:13});
  await post("/api/companies/"+company.id+"/contacts",{name:"架空の担当者",email:"buyer@example.invalid"});
  browser=await chromium.launch({headless:true});
  const context=await browser.newContext({viewport:{width:390,height:844}});
  let authorizations=0;
  const held=new Map();
  await context.route("**/*",async route=>{
    const u=new URL(route.request().url());
    if(u.origin===origin) {
      const match=/^\/api\/activity-sync\/(google|microsoft)\/pull$/.exec(u.pathname);
      if(match && held.has(match[1])) await held.get(match[1]).promise;
      return route.continue();
    }
    assert.ok(["accounts.google.com","login.microsoftonline.com"].includes(u.hostname));
    assert.equal(u.searchParams.get("response_type"),"code");
    assert.equal(u.searchParams.get("code_challenge_method"),"S256");
    assert.equal(u.searchParams.get("redirect_uri"),origin+"/oauth/activity-return");
    authorizations++;
    return route.fulfill({status:302,headers:{location:origin+"/oauth/activity-return?code=fixture&state="+u.searchParams.get("state")}});
  });
  let page=await context.newPage();const errors=[];
  const events=async()=>{
    const res=await fetch(origin+"/api/work-items?kind=event");
    assert.equal(res.status,200);
    return (await res.json()).items.map(({id,subject})=>({id,subject})).sort((a,b)=>a.id-b.id);
  };
  const subjects=async()=>(await events()).map(item=>item.subject).sort();
  const poll=check=>expect.poll(check,{timeout:20000,intervals:[50,100,200]});
  page.on("pageerror",e=>errors.push(e.message));
  await page.goto(origin);
  await page.locator('.workspace-navigation [data-workspace="home"]').click();
  await page.getByRole("button",{name:"メール・予定の連携",exact:true}).click();
  assert.equal(await page.locator("dialog[open]").evaluate(d=>d.getBoundingClientRect().right<=innerWidth),true);
  for(const name of ["Google","Microsoft"]) {
    await page.getByRole("button",{name:name+"に接続",exact:true}).click();
    await poll(async()=>{
      const {providers}=await (await fetch(origin+"/api/activity-sync")).json();
      const account=providers.find(p=>p.id===name.toLowerCase()).status;
      return account.connected && Number.isInteger(account.last_sync);
    }).toBe(true);
  }
  assert.equal(authorizations,2);
  const expected=["Google fixture event","Outlook fixture event"];
  await poll(subjects).toEqual(expected);
  const original=await events();
  let ids;
  for (const [revision,order] of [[1,["microsoft","google"]],[2,["google","microsoft"]]]) {
    await page.close();
    await writeFile(marker,String(revision));
    for(const provider of order) held.set(provider,Promise.withResolvers());
    try {
      page=await context.newPage();page.on("pageerror",e=>errors.push(e.message));
      const requested=order.map(provider=>page.waitForRequest(origin+"/api/activity-sync/"+provider+"/pull"));
      await page.goto(origin);await Promise.all(requested);
      // Both requests are deliberately paused; a premature waiter would have accepted these old rows.
      assert.deepEqual(await subjects(),[...expected].sort());
      for(const provider of order) {
        held.get(provider).resolve();held.delete(provider);
        if(provider==="microsoft") expected[1]="Outlook fixture event "+revision;
        else expected[2]="Closed-tab fixture event "+revision;
        await poll(subjects).toEqual([...expected].sort());
      }
      const rows=await events();
      assert.equal(rows.length,3);
      for(const entry of original) assert.ok(rows.some(row=>row.id===entry.id));
      if(ids) assert.deepEqual(rows.map(row=>row.id),ids);
      ids=rows.map(row=>row.id);
    } finally {
      for(const gate of held.values()) gate.resolve();
      held.clear();
    }
  }
  await page.locator('.workspace-navigation [data-workspace="home"]').click();
  await page.getByRole("button",{name:"メール・予定の連携",exact:true}).click();
  assert.equal(authorizations,2);
  const beforeDisconnect=await events();
  const card=page.locator("dialog section").filter({has:page.getByRole("heading",{name:"Outlook メール・予定表"})});
  await card.getByRole("button",{name:"接続を解除"}).click();await card.getByText("未接続",{exact:true}).waitFor();
  assert.deepEqual(await events(),beforeDisconnect);
  assert.equal(await readFile(marker+".full","utf8"),"","Both services must resume their saved cursor after each short gap");
  assert.deepEqual(errors,[]);
  console.log("Both consent dialogs, opposite delta arrival orders, stable event IDs after closing the tab, and disconnect passed on a 390px browser using synthetic providers.");
} finally {
  if(browser) await browser.close();
  if(child && child.exitCode===null && child.signalCode===null) {const stopped=new Promise(r=>child.once("exit",r));child.kill();await stopped;}
  await rm(scratch,{recursive:true,force:true});
}
