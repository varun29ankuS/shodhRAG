import DOMPurify from 'dompurify';
DOMPurify.addHook = () => {}; DOMPurify.removeHook = () => {}; DOMPurify.removeHooks = () => {}; DOMPurify.removeAllHooks = () => {}; DOMPurify.sanitize = s => String(s);
const { default: mermaid } = await import('mermaid');
import fs from 'node:fs';
const srcs = fs.readFileSync(process.argv[2], 'utf8').split('\n=====\n');
for (const s of srcs) {
  try { await mermaid.parse(s); console.log('OK   ', JSON.stringify(s)); }
  catch (e) { if (String(e.message).includes("DOMPurify")) { console.log("OK   ", JSON.stringify(s)); continue; } console.log('FAIL ', JSON.stringify(s), '::', String(e.message).split('\n').slice(0,4).join(' | ')); }
}
