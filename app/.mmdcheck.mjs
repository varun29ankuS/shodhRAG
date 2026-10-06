import DOMPurify from 'dompurify';
DOMPurify.addHook = () => {}; DOMPurify.removeHook = () => {}; DOMPurify.removeHooks = () => {}; DOMPurify.removeAllHooks = () => {}; DOMPurify.sanitize = s => String(s);
const { default: mermaid } = await import('mermaid');
import { BROKEN, VALID } from './tests/mermaidFixtures.ts';
import { repairMermaid } from './src/features/ask/visual/mermaidRepair.ts';
try { await mermaid.parse('flowchart LR;A-->B'); } catch {}
try { await mermaid.parse('flowchart LR;A-->B'); } catch {}
async function ok(s){ try { await mermaid.parse(s); return null } catch(e){ const m=String(e.message); return m.includes('DOMPurify')?null:m.split('\n').slice(0,4).join(' | ') } }
for (const f of BROKEN) {
  const a = await ok(f.source); const r = repairMermaid(f.source);
  const b = await ok(f.repaired);
  const c = r.source === f.repaired;
  const d = repairMermaid(r.source).source === r.source;
  console.log(f.name, '| orig fails:', !!a, '| expected parses:', !b, b??'', '| repair==expected:', c, '| idem:', d, r.fixes.join(','));
  if (!c) console.log('   GOT:', JSON.stringify(r.source));
}
for (const v of VALID) { const e = await ok(v); const r = repairMermaid(v); console.log('VALID parses:', !e, e??'', '| unchanged:', !r.changed, r.changed? JSON.stringify(r.source):''); }
