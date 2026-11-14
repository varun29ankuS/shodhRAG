import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './styles.css';

// The desktop's colour tokens (tokens.ts), before anything renders.
const tokens = document.createElement('style');
tokens.textContent = __SHODH_TOKENS__;
document.head.prepend(tokens);

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root');
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
