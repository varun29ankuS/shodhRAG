import mermaid from 'mermaid';
import { BROKEN } from './tests/mermaidFixtures.ts';
try { await mermaid.parse('flowchart LR;A-->B'); } catch {}
for (const f of BROKEN.slice(4,6)) { try { await mermaid.parse(f.source); console.log('OK', JSON.stringify(f.source)); } catch (e) { console.log('ERR', JSON.stringify(String(e?.message ?? e)).slice(0,300)); } }
