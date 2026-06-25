// Screen-local helpers for the Logs screen (data/text glue, not a UI primitive,
// so allowed by the SRCL-only rule). Parses the daemon's plain-text log tail into
// per-line records tagged with a severity level for filtering.

/** The severities the level filter can isolate. */
export type LogLevel = 'TRACE' | 'DEBUG' | 'INFO' | 'WARN' | 'ERROR';

export const LOG_LEVELS: LogLevel[] = ['TRACE', 'DEBUG', 'INFO', 'WARN', 'ERROR'];

export interface LogLine {
  index: number;
  text: string;
  /** Detected level, or null when no level token was found on the line. */
  level: LogLevel | null;
}

// Recognise the common level tokens emitted by tracing/log frameworks, whether
// bare (`INFO`), bracketed (`[WARN]`), or with the WARNING/ERR aliases.
const LEVEL_RE =
  /\b(TRACE|DEBUG|INFO|WARN(?:ING)?|ERR(?:OR)?)\b/i;

function normalizeLevel(token: string): LogLevel {
  const t = token.toUpperCase();
  if (t.startsWith('WARN')) return 'WARN';
  if (t.startsWith('ERR')) return 'ERROR';
  return t as LogLevel;
}

/** Detect the severity of a single log line, or null if none is present. */
export function detectLevel(line: string): LogLevel | null {
  const m = LEVEL_RE.exec(line);
  return m ? normalizeLevel(m[1]) : null;
}

/** Split raw log text into tagged, indexed lines (drops a trailing blank line). */
export function parseLogLines(text: string): LogLine[] {
  if (!text) return [];
  const rows = text.replace(/\r\n/g, '\n').split('\n');
  // A trailing newline yields a final empty element; drop only that one.
  if (rows.length && rows[rows.length - 1] === '') rows.pop();
  return rows.map((text, index) => ({ index, text, level: detectLevel(text) }));
}

/**
 * Filter parsed lines to a minimum severity. `null` (the "All" option) returns
 * everything; otherwise lines at or above the threshold are kept, plus any
 * untagged lines (continuations / stack traces) so context is not lost.
 */
export function filterByLevel(lines: LogLine[], min: LogLevel | null): LogLine[] {
  if (!min) return lines;
  const threshold = LOG_LEVELS.indexOf(min);
  return lines.filter((l) => {
    if (l.level === null) return true;
    return LOG_LEVELS.indexOf(l.level) >= threshold;
  });
}
