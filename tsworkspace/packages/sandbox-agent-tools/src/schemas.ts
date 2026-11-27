import { z } from 'zod';

/** Names of the tools, in the order they are offered to a model. */
export const TOOL_NAMES = [
  'run_command',
  'read_file',
  'write_file',
  'edit_file',
  'find_files',
  'background_process',
  'start_server',
  'preview_url',
  'checkpoint_files',
  'file_changes',
  'watch_files',
] as const;

export type ToolName = (typeof TOOL_NAMES)[number];

/**
 * Flat input schemas. No lease id (the bundle is bound to one sandbox), no argv
 * vectors (commands are shell strings) and no base64 (payloads are plain text).
 */
export const toolSchemas = {
  run_command: z.object({
    command: z.string().min(1).describe('Shell command line, run with `sh -c`. Pipes, redirects and && are allowed.'),
    cwd: z.string().optional().describe('Working directory. Defaults to the sandbox working directory.'),
    timeout_seconds: z
      .number()
      .int()
      .positive()
      .optional()
      .describe('Kill the command after this many seconds. Defaults to the bundle default.'),
  }),
  read_file: z.object({
    path: z.string().min(1).describe('File path, absolute or relative to the working directory.'),
    offset: z.number().int().positive().optional().describe('First line to return, starting at 1.'),
    limit: z.number().int().positive().optional().describe('Maximum number of lines to return.'),
  }),
  write_file: z.object({
    path: z.string().min(1).describe('File path, absolute or relative to the working directory.'),
    content: z.string().describe('Full new file content as text.'),
  }),
  edit_file: z.object({
    path: z.string().min(1).describe('File path, absolute or relative to the working directory.'),
    old_text: z.string().min(1).describe('Exact text to replace, including whitespace. Not a pattern.'),
    new_text: z.string().describe('Replacement text. May be empty to delete the old text.'),
    expected_occurrences: z
      .number()
      .int()
      .positive()
      .optional()
      .describe('How many times old_text must occur. Defaults to 1; the edit is refused on any other count.'),
  }),
  find_files: z.object({
    path: z.string().optional().describe('Directory to look in. Defaults to the working directory.'),
    pattern: z.string().optional().describe('Literal text to search for in file contents. Omit to list files instead.'),
    glob: z.string().optional().describe('Only consider file names matching this glob, for example `*.ts`.'),
    recursive: z.boolean().optional().describe('When listing, descend into subdirectories. Defaults to false.'),
    max_results: z.number().int().positive().optional().describe('Cap on returned entries or matches.'),
  }),
  background_process: z.object({
    action: z.enum(['start', 'status', 'stop']).describe('start a process, check on it, or stop it.'),
    name: z
      .string()
      .regex(/^[A-Za-z0-9._-]{1,64}$/)
      .describe('Short name that identifies the process in later calls.'),
    command: z.string().optional().describe('Shell command line. Required for `start`, ignored otherwise.'),
    cwd: z.string().optional().describe('Working directory for `start`.'),
  }),
  start_server: z.object({
    name: z
      .string()
      .regex(/^[A-Za-z0-9._-]{1,64}$/)
      .describe('Short name that identifies the server in later background_process calls.'),
    command: z.string().min(1).describe('Shell command line that starts the server, for example `npm run dev`.'),
    port: z.number().int().min(1).max(65535).describe('Port the server listens on inside the sandbox.'),
    path: z
      .string()
      .regex(/^\/\S*$/)
      .optional()
      .describe(
        'HTTP path that must answer before the server counts as ready, for example `/healthz`. Needs curl or wget in the sandbox.',
      ),
    cwd: z.string().optional().describe('Working directory for the command.'),
    timeout_seconds: z
      .number()
      .int()
      .positive()
      .optional()
      .describe('How long to wait for the server to answer. Defaults to 60 and is capped by the bundle.'),
  }),
  preview_url: z.object({
    port: z.number().int().min(1).max(65535).describe('Port a process inside the sandbox is listening on.'),
  }),
  checkpoint_files: z.object({
    path: z.string().optional().describe('Directory to record. Defaults to the working directory.'),
    hash: z
      .boolean()
      .optional()
      .describe('Also fingerprint content, so rewriting identical bytes is not reported as a change.'),
  }),
  file_changes: z.object({
    since: z
      .string()
      .optional()
      .describe('Checkpoint id from checkpoint_files. Omit to compare against the sandbox start.'),
    path: z.string().optional().describe('Only report paths under this directory.'),
  }),
  watch_files: z.object({
    path: z.string().min(1).describe('Directory or file to watch, absolute or relative to the working directory.'),
    recursive: z.boolean().optional().describe('Watch subdirectories too. Defaults to false.'),
    duration_seconds: z
      .number()
      .int()
      .positive()
      .optional()
      .describe('Stop watching after this many seconds. Defaults to 10 and is capped by the bundle.'),
    max_events: z
      .number()
      .int()
      .positive()
      .optional()
      .describe('Stop once this many events were seen. Defaults to 50.'),
  }),
} as const;

export type ToolSchemas = typeof toolSchemas;
export type ToolInput<N extends ToolName> = z.output<ToolSchemas[N]>;

/**
 * Descriptions say what the tool is for and, just as important, when another
 * tool or no tool is the better choice.
 */
export const toolDescriptions: Record<ToolName, string> = {
  run_command:
    'Run a shell command in the sandbox and wait for it to finish. Returns a verdict line, then stdout and stderr. ' +
    'Do not use it to read, write, edit or search files (use the file tools, which are cheaper and safer), ' +
    'and do not use it for servers or anything that does not exit (use background_process).',
  read_file:
    'Read a text file with numbered lines. Use offset and limit to page through large files. ' +
    'Do not use it to list directories (use find_files) or to read binary files.',
  write_file:
    'Create a file or replace its whole content, creating parent directories. ' +
    'Do not use it for small changes to an existing file (use edit_file), because it overwrites everything.',
  edit_file:
    'Replace exact text in an existing file. The edit is refused, and nothing is written, unless old_text occurs ' +
    'exactly the expected number of times. Do not use it to create files (use write_file) or for pattern-based rewrites.',
  find_files:
    'List files in a directory, or search file contents for literal text. Returns bounded results. ' +
    'Do not use it to read a file once you know its path (use read_file).',
  background_process:
    'Start, check or stop a long-running named process such as a dev server or watcher. ' +
    'Do not use it for commands that finish on their own (use run_command). Output of a process is reported once it exits.',
  start_server:
    'Start a dev server in the background, wait until its port (and optional HTTP path) answers, and return its preview URL. ' +
    'If the process exits first or the wait times out, the verdict carries its recent output. ' +
    'Do not use it for commands that finish on their own (use run_command); stop or check the server later with background_process.',
  preview_url:
    'Get the address where a port opened inside the sandbox can be reached. ' +
    'Call it after background_process reports the server is running; do not guess URLs.',
  checkpoint_files:
    'Record the current state of a directory so file_changes can later report what changed since. ' +
    'Do not use it to read files, and call it before the work you want to measure, not after.',
  file_changes:
    'List files added, modified and deleted since a checkpoint (or since the sandbox started). ' +
    'Do not use it to see file contents (use read_file) or to wait for changes (use watch_files).',
  watch_files:
    'Watch a path for a short, bounded time and report the create, modify and delete events seen. ' +
    'Do not use it for a one-off check of what changed (use file_changes); it returns when time or the event limit is reached.',
};
