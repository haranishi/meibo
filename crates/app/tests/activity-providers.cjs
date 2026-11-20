"use strict";
const test=require("node:test");
const assert=require("node:assert/strict");
const {run,scopes}=require("../connector/providers.cjs");
const success=value=>({ok:true,json:async()=>value});
const failed=status=>({ok:false,status,json:async()=>({})});
const fake={client_id:"synthetic-client",secret:"synthetic-secret",redirect:"https://crm.example.test/oauth/activity-return"};
const date="2026-11-20T10:00:00Z";

for (const provider of ["google","microsoft"]) test(provider+" uses code, PKCE and delegated read-only scopes",async()=>{
  const start=await run({...fake,provider,operation:"begin"},()=>assert.fail());
  const dest=new URL(start.url);
  assert.equal(dest.searchParams.get("response_type"),"code");
  assert.equal(dest.searchParams.get("code_challenge_method"),"S256");
  assert.equal(dest.searchParams.get("state"),start.state);
  assert.ok(!dest.href.includes(start.verifier)); assert.ok(!dest.href.includes(fake.secret));
  const calls=[];
  const connected=await run({...fake,provider,operation:"finish",code:"synthetic-code",verifier:start.verifier},async(uri,opt)=>{
    calls.push(String(uri));
    if(opt.method==="POST"){
      assert.equal(opt.body.get("code_verifier"),start.verifier);
      return success({access_token:"synthetic-access",refresh_token:"synthetic-refresh",expires_in:3600,scope:scopes[provider].join(" ")});
    }
    if(String(uri).includes("/profile?")) return success({emailAddress:"agent@example.invalid",historyId:"10"});
    return success({sub:"subject-0",email:"agent@example.invalid"});
  });
  assert.equal(connected.account,"subject-0");assert.equal(connected.address,"agent@example.invalid");
  assert.ok(calls.every(u=>u.startsWith("https://")));
  const refreshed=await run({...fake,provider,operation:"refresh",refresh:"old-synthetic"},async(u,opt)=>{
    assert.equal(opt.body.get("grant_type"),"refresh_token");
    return success({access_token:"new-short",refresh_token:"rotated",expires_in:3600,scope:scopes[provider].join(" ")});
  });
  assert.equal(refreshed.refresh,"rotated");
  await assert.rejects(run({...fake,provider,operation:"refresh",refresh:"old"},async()=>success({access_token:"short",scope:"openid"})),{code:"consent"});
  if (provider==="microsoft") {
    await assert.rejects(run({...fake,provider,operation:"finish",code:"synthetic",verifier:start.verifier},
      async(_url,options)=>success(options.method==="POST" ? {access_token:"short",refresh_token:"refresh",expires_in:3600} : {sub:"user-without-email"})),{code:"consent"});
  }
});

const request=(provider,stream,cursor={})=>({operation:"page",provider,stream,cursor,access:"synthetic",
  since:Date.parse("2026-11-18T00:00:00Z"),address:"agent@example.invalid"});
test("Gmail replays changes after a closed tab, paginates and projects only headers",async()=>{
  const urls=[];
  const result=await run(request("google","inbox",{history:"10"}),async(uri,options)=>{
    const u=new URL(uri); urls.push(u);
    assert.equal(options.redirect,"error");assert.equal(options.headers.Authorization,"Bearer synthetic");
    if(u.pathname.endsWith("/history"))return success({history:[{messagesAdded:[{message:{id:"m1"}}]}],historyId:"12",nextPageToken:"next"});
    assert.equal(u.searchParams.get("format"),"metadata");
    assert.deepEqual(u.searchParams.getAll("metadataHeaders"),["From","To","Cc","Subject"]);
    return success({id:"m1",internalDate:String(Date.parse(date)),labelIds:["INBOX"],body:"must remain private",payload:{headers:[
      {name:"From",value:'"decoy@example.invalid" <real@example.invalid>'},{name:"To",value:"agent@example.invalid"},{name:"Subject",value:"Meeting"}]}});
  });
  assert.equal(urls[0].searchParams.get("startHistoryId"),"10");
  assert.equal(result.more,true);assert.equal(result.cursor.history,"10");assert.equal(result.cursor.page,"next");
  assert.deepEqual(result.items,[{id:"m1",kind:"mail",subject:"Meeting",at:Date.parse(date)/1000,addresses:["real@example.invalid"]}]);
  const final=await run(request("google","inbox",result.cursor),async(uri)=>{
    assert.equal(new URL(uri).searchParams.get("pageToken"),"next");return success({historyId:"12"});
  });
  assert.deepEqual(final.cursor,{history:"12"});
  const fallback=await run(request("google","inbox",{history:"expired"}),async uri=>String(uri).includes("/profile?")?success({historyId:"20"}):failed(404));
  assert.deepEqual(fallback.cursor,{history:"20",baseline:true});
  const scan=await run(request("google","inbox",fallback.cursor),async uri=>{
    assert.equal(new URL(uri).searchParams.has("q"),false);return success({messages:[]});
  });
  assert.deepEqual(scan.cursor,{history:"20"});
});
test("Google primary calendar handles all-day dates, cancellation, hide-private, paging and expired cursor",async()=>{
  const appointment={id:"event",summary:"Briefing",start:{date:"2026-11-20"},end:{date:"2026-11-21"},
    attendees:[{email:"c@example.invalid"},{self:true,email:"agent@example.invalid"}]};
  const first=await run(request("google","calendar"),async uri=>{
    const u=new URL(uri);assert.ok(u.pathname.endsWith("/primary/events"));assert.ok(!u.searchParams.get("fields").includes("description"));
    return success({timeZone:"Asia/Tokyo",items:[appointment,{id:"private",visibility:"private",summary:"private"}],nextPageToken:"page2"});
  });
  assert.equal(first.items[0].at,Date.parse("2026-11-19T15:00:00Z")/1000);
  assert.deepEqual(first.items[1],{id:"private",kind:"removed_event"});
  const last=await run(request("google","calendar",first.cursor),async()=>success({items:[{id:"event",status:"cancelled"}],nextSyncToken:"delta"}));
  assert.equal(last.reconcile,true);assert.equal(first.epoch,last.epoch);
  assert.deepEqual(last.items,[{id:"event",kind:"removed_event"}]);
  const incremental=await run(request("google","calendar",last.cursor),async()=>success({items:[{...appointment,id:"later"}],nextSyncToken:"newer"}));
  assert.equal(incremental.reconcile,false);assert.equal(incremental.items[0].id,"later");
  const unchanged=await run(request("google","calendar",incremental.cursor),async()=>success({items:[],nextSyncToken:"newer"}));
  assert.equal(unchanged.reconcile,false);assert.deepEqual(unchanged.items,[]);
  const reset=await run(request("google","calendar",last.cursor),async uri=>{
    const u=new URL(uri);assert.equal(u.searchParams.get("syncToken"),"delta");assert.ok(!u.searchParams.has("timeMin"));return failed(410);
  });
  assert.deepEqual(reset,{items:[],cursor:{},more:true});
});
test("Outlook inbox and sent messages are metadata-only and IDs survive folder moves",async()=>{
  for (const stream of ["inbox","sent"]) {
    const result=await run(request("microsoft",stream),async(uri,options)=>{
      const u=new URL(uri);assert.ok(u.pathname.includes(stream==="sent"?"/sentitems/":"/inbox/"));
      assert.ok(!u.searchParams.get("$select").includes("body"));assert.ok(options.headers.Prefer.includes("ImmutableId"));
      return success({value:[{id:"unique",subject:"Hello",receivedDateTime:date,sentDateTime:date,
        from:{emailAddress:{address:"c@example.invalid"}},toRecipients:[{emailAddress:{address:"agent@example.invalid"}}],
        body:{content:"secret"}}],"@odata.deltaLink":u.origin+u.pathname+"?$deltatoken=safe"});
    });
    assert.equal(result.items.length,1);assert.deepEqual(result.items[0].addresses,["c@example.invalid"]);
    assert.ok(!JSON.stringify(result).includes("secret"));
    await assert.rejects(run(request("microsoft",stream,result.cursor),async()=>success({value:[],"@odata.nextLink":"https://other.example.test/v1.0/me/messages/delta"})));
  }
});
test("Outlook primary calendar follows delta, revokes removed records and supports rebootstrap",async()=>{
  const page=await run(request("microsoft","calendar"),async uri=>{
    const u=new URL(uri);assert.ok(u.searchParams.has("startDateTime"));
    return success({value:[{id:"e",subject:"Call",start:{dateTime:"2026-11-20T10:00:00",timeZone:"UTC"},
      end:{dateTime:"2026-11-20T11:00:00",timeZone:"UTC"},attendees:[{emailAddress:{address:"biz@example.invalid"}}]},
      {id:"cancelled","@removed":{reason:"deleted"}}],"@odata.deltaLink":u.origin+u.pathname+"?$deltatoken=gone"});
  });
  assert.equal(page.items[0].at,Date.parse(date)/1000);assert.equal(page.items[0].kind,"event");
  assert.equal(page.items[1].kind,"removed_event");assert.equal(page.reconcile,true);
  const unchanged=await run(request("microsoft","calendar",page.cursor),async()=>success({value:[],"@odata.deltaLink":page.cursor.link}));
  assert.equal(unchanged.reconcile,false);assert.deepEqual(unchanged.items,[]);
  const reset=await run(request("microsoft","calendar",page.cursor),async()=>failed(410));assert.equal(reset.more,true);
  for(const status of [400,404]) {
    const missing=await run(request("microsoft","calendar",page.cursor),async()=>({
      ok:false,status,json:async()=>({error:{code:"SyncStateNotFound",message:"private error"}})
    }));
    assert.deepEqual(missing,{items:[],cursor:{},more:true});
  }
});
