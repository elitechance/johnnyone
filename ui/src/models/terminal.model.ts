export interface TerminalScreen {
  sessionId: string;
  paneId: string;
  cursor: number;
  content: string;
  cursorX: number;
  cursorY: number;
  historyLines: number;
  rows: number;
  cols: number;
  status: string;
}
