// Runs only after an explicit connection. No provider endpoint or credentials come from the browser.
"use strict";
const { createHash, randomBytes, randomUUID } = require("node:crypto");
const googleAuth = "https://accounts.google.com/o/oauth2/v2/auth";
const microsoftAuth = "https://login.microsoftonline.com/common/oauth2/v2.0/";
const graph = "https://graph.microsoft.com/v1.0/me/";
const gmail = "https://gmail.googleapis.com/gmail/v1/users/me/";
const calendar = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
const scopes = {
  google: ["openid", "https://www.googleapis.com/auth/gmail.metadata", "https://www.googleapis.com/auth/calendar.events.owned.readonly"],
  microsoft: ["openid", "email", "offline_access", "Mail.ReadBasic", "Calendars.ReadBasic"]
};
const days = 86400000;
const iso = ms => new Date(ms).toISOString();
const seconds = v => Math.floor(Date.parse(v) / 1000);
function url(base, fields) {
  const u = new URL(base);
  for (const [name, value] of Object.entries(fields)) if (value != null) u.searchParams.set(name, value);
  return u;
}
function failure(code, status) { return Object.assign(new Error(code), { code, status }); }
async function json(fetcher, target, options = {}) {
  const response = await fetcher(target, { ...options, redirect: "error", signal: AbortSignal.timeout(8000) });
  if (!response.ok) {
    const error = response.status >=400 && response.status<500 ? await response.json().catch(() => ({})) : {};
    if (String(error.error?.code).toLowerCase()==="syncstatenotfound") throw failure("expired_cursor",response.status);
    throw failure(error.error === "invalid_grant" || response.status === 401 ? "reauth" : "provider", response.status);
  }
  return response.json();
}
function reader(fetcher, token) {
  return (target, headers) => json(fetcher, target, {headers: { Authorization: "Bearer " + token, ...headers }});
}
function begin(a) {
  const verifier = randomBytes(32).toString("base64url");
  const state = randomBytes(32).toString("hex");
  const location = url(a.provider === "google" ? googleAuth : microsoftAuth + "authorize", {
    client_id:a.client_id, redirect_uri:a.redirect, response_type:"code", state,
    scope:scopes[a.provider].join(" "), code_challenge_method:"S256",
    code_challenge:createHash("sha256").update(verifier).digest("base64url"),
    ...(a.provider === "google" ? {access_type:"offline",prompt:"consent select_account"} : {prompt:"select_account",response_mode:"query"})
  });
  return { state, verifier, url:location.href };
}
async function tokens(a, fetcher) {
  const base = { client_id:a.client_id, client_secret:a.secret };
  const grant = a.operation === "finish" ?
    {grant_type:"authorization_code",redirect_uri:a.redirect,code_verifier:a.verifier,code:a.code} :
    {grant_type:"refresh_token",refresh_token:a.refresh};
  const token = await json(fetcher, a.provider==="google" ? "https://oauth2.googleapis.com/token" : microsoftAuth+"token", {
    method:"POST",body:new URLSearchParams({...base,...grant})
  });
  if (!token.access_token || (!token.refresh_token && !a.refresh)) throw failure("reauth");
  // Google allows granular consent; Microsoft returns Graph scope names with or without its resource prefix.
  if (token.scope) {
    const granted = new Set(token.scope.toLowerCase().replaceAll("https://graph.microsoft.com/","").split(" "));
    const missing = scopes[a.provider].filter(s => !["email","openid","offline_access"].includes(s)).some(s => !granted.has(s.toLowerCase()));
    if (missing) throw failure("consent");
  }
  return { access:token.access_token, refresh:token.refresh_token || a.refresh,
    expires:Date.now()+Math.max(0,(Number(token.expires_in)||0)-90)*1000 };
}
function normalizeAddresses(addresses, own) {
  const result = new Set();
  for (const value of addresses) if (typeof value==="string") {
    const email = value.trim().toLowerCase();
    if (email && email!==own && email.length<=320) result.add(email);
  }
  // Truncating a recipient list could incorrectly turn two matches into one.
  return result.size>256 ? [] : [...result];
}
function subject(value) { return Array.from(String(value || "").replace(/[\u0000-\u001f\u007f]/g," ")).slice(0,200).join(""); }
function headerAddresses(value) {
  // Ignore quoted display names and comments; an address-looking display name is not a recipient.
  const bare = String(value).replace(/"(?:\\.|[^"\\])*"|\([^()]*\)/g, "");
  return bare.split(/[,;]/).flatMap(part => {
    const bracket = part.match(/<([^<>]+)>/);
    const address = (bracket ? bracket[1] : part).trim();
    return /^[^\s<>,;:@]+@[^\s<>,;:@]+$/.test(address) ? [address] : [];
  });
}
function googleMail(m, owner, since) {
  if ((m.labelIds||[]).some(label=>["SPAM","TRASH","DRAFT"].includes(label))) return null;
  const at = Math.floor(Number(m.internalDate)/1000);
  if (!Number.isFinite(at) || at<Math.floor(since/1000)) return null;
  const headers = m.payload?.headers || [];
  const addresses = headers.filter(h=>["from","to","cc"].includes(h.name.toLowerCase())).flatMap(h=>headerAddresses(h.value));
  return { id:m.id,kind:"mail",at,subject:subject(headers.find(h=>h.name.toLowerCase()==="subject")?.value),
    addresses:normalizeAddresses(addresses,owner) };
}
function eventRecord(e, provider, owner) {
  const removed = {id:e.id,kind:"removed_event"};
  if (e["@removed"] || e.isCancelled || e.status==="cancelled" || ["private","confidential","personal"].includes(e.visibility || e.sensitivity) || e.attendeesOmitted) return removed;
  const google = provider==="google";
  const instant = o => {
    if (!o) return NaN;
    if (o.date) {
      // Google all-day boundaries are calendar-local midnights, even though the response uses UTC for timed events.
      const midnight=Date.parse(o.date+"T00:00:00Z");
      const zone=o.timeZone || e.calendarZone || "UTC";
      const offset=ms=> {
        const parts=new Intl.DateTimeFormat("sv-SE",{timeZone:zone,timeZoneName:"longOffset"}).formatToParts(ms);
        const match=parts.find(p=>p.type==="timeZoneName")?.value.match(/GMT([+-])(\d\d):(\d\d)/);
        return match ? (match[1]==="+"?1:-1)*(Number(match[2])*60+Number(match[3]))*60000 : 0;
      };
      return Math.floor((midnight-offset(midnight-offset(midnight)))/1000);
    }
    if (!o.dateTime) return NaN;
    if (!google && !/[zZ]|[+-]\d\d:\d\d$/.test(o.dateTime) && o.timeZone!=="UTC") return NaN;
    return seconds(!google && !/[zZ]|[+-]\d\d:\d\d$/.test(o.dateTime) ? o.dateTime+"Z" : o.dateTime);
  };
  const at=instant(e.start), end=instant(e.end);
  if (!Number.isFinite(at) || !Number.isFinite(end) || end<=at) return null;
  const people=[e.organizer,...(e.attendees||[])].filter(Boolean);
  return {id:e.id,kind:"event",at,end,subject:subject(google?e.summary:e.subject),
    addresses:normalizeAddresses(people.filter(p=>!p.self).map(p=>google?p.email:p.emailAddress?.address),owner)};
}
async function gmailPage(read, a, c) {
  const owner=a.address, since=a.since;
  let response;
  if (c.history && !c.baseline) {
    try { response=await read(url(gmail+"history",{startHistoryId:c.history,pageToken:c.page,historyTypes:"messageAdded",maxResults:8,
      fields:"nextPageToken,historyId,history(messagesAdded/message/id)"})); }
    catch(e) {
      if (e.status!==404) throw e;
      // Save a high-water mark before scanning, so messages arriving during the scan will be replayed.
      const profile=await read(url(gmail+"profile",{fields:"historyId"}));
      return {items:[],cursor:{history:profile.historyId,baseline:true},more:true};
    }
  } else {
    response=await read(url(gmail+"messages",{pageToken:c.page,maxResults:8,fields:"nextPageToken,messages/id"}));
  }
  const ids=[...new Set(c.baseline ? (response.messages||[]).map(m=>m.id) :
    (response.history||[]).flatMap(h=>(h.messagesAdded||[]).map(add=>add.message.id)))];
  // Some history records contain many message IDs. Keep the offset until each is committed.
  const offset=c.offset || 0;
  const items=(await Promise.all(ids.slice(offset,offset+8).map(async id=>{
    const u=url(gmail+"messages/"+encodeURIComponent(id),{format:"metadata",fields:"id,internalDate,labelIds,payload/headers"});
    for (const name of ["From","To","Cc","Subject"]) u.searchParams.append("metadataHeaders",name);
    try { return googleMail(await read(u),owner,since); }
    catch(e) { if(e.status!==404) throw e; return null; }
  }))).filter(Boolean);
  if (offset+8<ids.length) return {items,cursor:{...c,offset:offset+8},more:true};
  const more=Boolean(response.nextPageToken);
  return {items,more,cursor:more ? {...c,page:response.nextPageToken,offset:0} : {history:c.baseline ? c.history : response.historyId}};
}
function checkedGraph(link,events) {
  const u=new URL(link);
  const path=u.pathname.toLowerCase();
  const resource=events ? path.includes("/calendarview/") : path.includes("/mailfolders") && path.includes("/messages/");
  if (u.origin!=="https://graph.microsoft.com" || u.username || u.password || u.hash || !path.startsWith("/v1.0/") ||
      !resource || !path.includes("delta") || u.href.length>32768) throw failure("provider");
  return u;
}
function calendarRange(a,c) {
  // A renewed view starts at the original boundary, including time spent away from the CRM.
  if (!c.until || (!c.inProgress && c.until<Date.now()+180*days)) {
    return {since:a.since-30*days,until:Date.now()+187*days,epoch:randomUUID(),baseline:true};
  }
  return c;
}
function googleCalendar(read,a,previous) {
  const c=calendarRange(a,previous);
  const u=url(calendar,{singleEvents:true,showDeleted:true,maxResults:30,pageToken:c.page,syncToken:c.sync,
    timeMin:c.sync?null:iso(c.since),timeMax:c.sync?null:iso(c.until),
    fields:"timeZone,nextPageToken,nextSyncToken,items(id,status,visibility,summary,start,end,attendeesOmitted,organizer(email,self),attendees(email,self))"});
  return read(u).then(data=>{
    const items=(data.items||[]).map(e=>eventRecord({...e,calendarZone:data.timeZone},"google",a.address)).filter(Boolean);
    const more=Boolean(data.nextPageToken);
    if (!more && !data.nextSyncToken) throw failure("provider");
    return {items,more,cursor:{...c,page:data.nextPageToken,sync:more?c.sync:data.nextSyncToken,inProgress:more,baseline:more&&c.baseline},
      epoch:c.epoch,reconcile:!more&&Boolean(c.baseline)};
  }).catch(e=>{if(e.status===410) return {items:[],cursor:{},more:true}; throw e;});
}
async function graphPage(read,a,stream,prior) {
  const events=stream==="calendar";
  const c=events ? calendarRange(a,prior) : prior;
  const endpoint=events ? url(graph+"calendarView/delta",{startDateTime:iso(c.since),endDateTime:iso(c.until)}) :
    url(graph+"mailFolders/"+(stream==="sent"?"sentitems":"inbox")+"/messages/delta",{
      "$select":"id,subject,from,toRecipients,ccRecipients,receivedDateTime,sentDateTime,isDraft"
    });
  let data;
  try { data=await read(c.link ? checkedGraph(c.link,events) : endpoint,{Prefer:'odata.maxpagesize=30, outlook.timezone="UTC", IdType="ImmutableId"'}); }
  catch(e) { if(e.status===410 || e.code==="expired_cursor") return {items:[],cursor:{},more:true}; throw e; }
  const more=Boolean(data["@odata.nextLink"]);
  const link=checkedGraph(data["@odata.nextLink"]||data["@odata.deltaLink"],events).href;
  const items=[];
  for (const m of data.value || []) {
    if (events) { const event=eventRecord(m,"microsoft",a.address); if(event) items.push(event); continue; }
    if (m["@removed"] || m.isDraft) continue;
    const at=seconds(stream==="sent"?m.sentDateTime:m.receivedDateTime);
    if (!Number.isFinite(at) || at<Math.floor(a.since/1000)) continue;
    const people=[m.from,...(m.toRecipients||[]),...(m.ccRecipients||[])].filter(Boolean);
    items.push({id:m.id,kind:"mail",subject:subject(m.subject),at,addresses:normalizeAddresses(people.map(p=>p.emailAddress?.address),a.address)});
  }
  return {items,more,cursor:{...c,link,inProgress:more,baseline:more&&c.baseline},
    ...(events ? {epoch:c.epoch,reconcile:!more&&Boolean(c.baseline)} : {})};
}
async function run(a, fetcher=fetch) {
  if (!Object.hasOwn(scopes,a.provider)) throw failure("input");
  if (a.operation==="begin") return begin(a);
  if (a.operation==="finish") {
    const t=await tokens(a,fetcher), read=reader(fetcher,t.access);
    const profile=await read(a.provider==="google"?url(gmail+"profile",{fields:"emailAddress,historyId"}):"https://graph.microsoft.com/oidc/userinfo");
    const account=a.provider==="google"?(await read("https://openidconnect.googleapis.com/v1/userinfo")).sub:profile.sub;
    const address=String(a.provider==="google"?profile.emailAddress:profile.email || "").trim().toLowerCase();
    if (!account || !address.includes("@") || (a.provider==="google" && !profile.historyId)) throw failure("consent");
    return {...t,account,address,initial:a.provider==="google"?{history:profile.historyId}:{}};
  }
  if (a.operation==="refresh") return tokens(a,fetcher);
  if (a.operation!=="page") throw failure("input");
  const read=reader(fetcher,a.access);
  if (a.provider==="microsoft") return graphPage(read,a,a.stream,a.cursor);
  return a.stream==="calendar"?googleCalendar(read,a,a.cursor):gmailPage(read,a,a.cursor);
}
module.exports = {run,scopes};
if (process.argv[1]==="meibo-activity-worker") {
  (async()=>{
    const deadline=setTimeout(()=>process.exit(1),24000); deadline.unref();
    try {
      const parts=[]; for await (const chunk of process.stdin) parts.push(chunk);
      const result=await run(JSON.parse(Buffer.concat(parts).toString("utf8")));
      process.stdout.write(JSON.stringify({result}));
    } catch(error) {
      const code=["reauth","consent"].includes(error.code)?error.code:"provider";
      process.stdout.write(JSON.stringify({error:code}));
    } finally {clearTimeout(deadline);}
  })();
}
