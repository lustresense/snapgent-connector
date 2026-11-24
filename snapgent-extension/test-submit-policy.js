// Exercise real adapter submit routines with deterministic DOM substitutes.
const fs = require("fs");
const assert = require("assert");
const names = ["chatgpt","claude","grok","mistral","ptero","qwen-coder","deepseek","glm","gemini","kimi","qwen","arena","meta"];
async function scenario(name, mode) {
  const source = fs.readFileSync(__dirname + "/providers/" + name + ".js", "utf8");
  const i = source.indexOf("async function typeAndSend(");
  const j = source.indexOf("\n  }", i) + 4;
  assert(i >= 0 && j > i, name + ": submit routine missing");
  let draft = "", users = 0, clicks = 0, enters = 0, allowed = mode !== "stopped-before";
  let submittedText = "";
  const feedback = "feedback line\nsecond";
  const land = () => { submittedText = draft; draft = ""; users++; };
  class Editor {
    get value() { return draft; }
    set value(t) { draft = t; }
    get textContent() { return draft; }
    set textContent(t) { draft = t; }
    getAttribute() { return null; }
    hasAttribute() { return false; }
    setAttribute() {}
    removeAttribute() {}
    focus() {}
    getClientRects() { return [{}]; }
    dispatchEvent(event) { if (event.type === "keydown" && event.key === "Enter") { enters++; land(); } }
  }
  class Key { constructor(type, options) { this.type = type; Object.assign(this, options); } }
  const editor = new Editor();
  const button = {disabled:mode === "disabled", getAttribute:()=>null, focus(){}, click(){
    clicks++;
    if (mode === "swallowed-click" && clicks === 1) return;
    land();
  }};
  const setText = (_,t) => { draft = t; return true; };
  const env = {
    getEditor:()=>editor, editorText:()=>draft, userCount:()=>users, writeText:setText,
    setTextareaValue:setText, setEditorText:setText, waitEditorReady:async()=>editor,
    composerHolds:(_,t)=>draft===t, truncateForSend:t=>t,
    _locked:false, _injecting:false, HTMLTextAreaElement:Editor, HTMLInputElement:Editor,
    stagedSendImages:new WeakSet(), internalSendKeys:new WeakSet(), _nativeSetter:null,
    rememberSentResponse(){}, tagImages(){}, pendingCount:()=>0, _attachedImages:null,
    hasPendingAttachment:()=>false, attachImages:async()=>true, clearAttachments(){},
    composerFrame:()=>null, visibleEditor:()=>editor, writeEl:()=>editor, isValueEditor:()=>true,
    sendButton:()=>button.disabled && ["kimi","qwen"].includes(name)?null:button,
    document:{querySelector:()=>button,querySelectorAll:()=>[],activeElement:editor},
    window:{getSelection:()=>null}, S:{editor:"",sendBtn:""},
    stopButton:()=>null, isStopBtn:()=>false, isHardGenerating:()=>false,
    isBusyNow:()=>false, genActive:()=>false, unwedgeStopPersistently:async()=>{},
    diag(){}, sleep:async()=>{}, waitFor:async pred=>{
      if(mode==="stopped-during-wait") allowed=false;
      return !!pred();
    },
    pressEnter:ed=>ed.dispatchEvent(new Key("keydown",{key:"Enter"})),
    KeyboardEvent:Key, Event:Key
  };
  const fn = new Function("env", "with(env) { return (" + source.slice(i,j) + "); }")(env);
  const result = await fn(feedback,null,()=>allowed,mode==="retry"?1:0);
  if (mode === "swallowed-click") {
    assert.strictEqual(result,true,name+": first click attempted");
    assert.strictEqual(draft,feedback,name+": first click left full draft");
    assert.strictEqual(users,0,name+": first click did not submit");
    assert.strictEqual(await fn(feedback,null,()=>allowed,1),true,name+": fallback attempted");
    assert.strictEqual(clicks,1,name+": no repeat of swallowed click");
    assert.strictEqual(enters,1,name+": automatic Enter fallback");
    assert.strictEqual(users,1,name+": exactly one actual message");
    assert.strictEqual(submittedText,feedback,name+": fallback sent full feedback");
  } else if(mode==="ready" || mode==="retry") {
    assert.strictEqual(result,true,name+": "+mode+" attempted");
    assert.strictEqual(clicks+enters,1,name+": exactly one dispatch");
    assert.strictEqual(submittedText,feedback,name+": complete feedback");
  } else {
    assert.strictEqual(result,false,name+": "+mode+" rejected");
    assert.strictEqual(clicks+enters,0,name+": "+mode+" never dispatched");
  }
}
async function main() {
  for(const name of names) {
    for(const mode of ["ready","retry","disabled","stopped-before","stopped-during-wait","swallowed-click"]) {
      await scenario(name,mode);
      console.log("PASS "+name+" "+mode);
    }
  }
}
main().catch(error=>{console.error(error);process.exitCode=1;});
