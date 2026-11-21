import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/desk.css';
import { DeskApp } from './app';

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Missing #root mount element.');
}

createRoot(rootElement).render(
  <StrictMode>
    <DeskApp />
  </StrictMode>,
);
