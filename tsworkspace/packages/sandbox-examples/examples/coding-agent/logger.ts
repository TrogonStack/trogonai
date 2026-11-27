/**
 * Progress lines for the CLI. They go to stderr so stdout carries only the report, and `info` is
 * silenced when `NODE_ENV=production`. Errors are always written.
 */
const quiet = (): boolean => process.env.NODE_ENV === 'production';

export const logger = {
  info(message: string): void {
    if (!quiet()) process.stderr.write(`info: ${message}\n`);
  },
  error(message: string): void {
    process.stderr.write(`error: ${message}\n`);
  },
};
