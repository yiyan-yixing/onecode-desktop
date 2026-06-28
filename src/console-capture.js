// Capture console.log to a visible floating panel
const logPanel = document.createElement('div');
logPanel.id = 'logPanel';
logPanel.style.cssText = 'position:fixed;bottom:30px;right:8px;width:450px;max-height:200px;overflow-y:auto;background:rgba(0,0,0,.9);color:#0f8;font:11px/1.3 monospace;z-index:99999;padding:8px;border:2px solid #0f8;border-radius:6px;pointer-events:auto;';
document.addEventListener('DOMContentLoaded', () => document.body.appendChild(logPanel));

const origLog = console.log;
console.log = function(...args) {
  origLog.apply(console, args);
  const line = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
  const div = document.createElement('div');
  div.textContent = line;
  div.style.borderBottom = '1px solid #333';
  logPanel.appendChild(div);
  // Keep only last 30 lines
  while (logPanel.children.length > 30) logPanel.removeChild(logPanel.firstChild);
  logPanel.scrollTop = logPanel.scrollHeight;
};
