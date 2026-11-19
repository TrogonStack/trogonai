import { z } from 'zod';

import { SandboxWsError } from './errors.ts';
import { FILE_OP_MAX_ENTRIES } from './fileops.ts';

const filePath = z
  .string()
  .min(1)
  .max(4096)
  .refine((value) => !value.includes('\0'));
const fileUser = z.string().min(1).max(128);
const fileText = z.string().max(16 * 1024 * 1024);
const entryLimit = z.number().int().min(1).max(FILE_OP_MAX_ENTRIES);

export const fileListInputSchema = z.strictObject({
  path: filePath,
  recursive: z.boolean().optional(),
  maxEntries: entryLimit.optional(),
  user: fileUser.optional(),
});

export const fileEditInputSchema = z.strictObject({
  path: filePath,
  find: fileText.min(1),
  replace: fileText,
  expectedOccurrences: z.number().int().min(0).max(0xffff_ffff).optional(),
  user: fileUser.optional(),
});

export const fileSearchInputSchema = z.strictObject({
  path: filePath,
  query: fileText.min(1),
  glob: z.string().min(1).max(4096).optional(),
  regex: z.boolean().optional(),
  caseInsensitive: z.boolean().optional(),
  maxResults: entryLimit.optional(),
  maxFileBytes: z
    .number()
    .int()
    .min(1)
    .max(16 * 1024 * 1024)
    .optional(),
  user: fileUser.optional(),
});

export const structuredFileStatSchema = z.object({
  path: filePath,
  name: z.string().min(1).max(4096),
  kind: z.enum(['file', 'directory', 'symlink', 'other']),
  size: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
  mode: z.number().int().min(0).max(0o7777).optional(),
  modified_unix_ms: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).optional(),
  symlink_target: z.string().max(4096).optional(),
});

export const fileListingSchema = z.object({
  entries: z.array(structuredFileStatSchema).max(FILE_OP_MAX_ENTRIES),
  truncated: z.boolean(),
});

export const fileEditResultSchema = z.object({
  occurrences: z.number().int().min(0).max(0xffff_ffff),
  size: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
});

export const fileSearchResultSchema = z.object({
  matches: z
    .array(
      z.object({
        path: filePath,
        line: z.number().int().positive().max(Number.MAX_SAFE_INTEGER).optional(),
        text: fileText.optional(),
      }),
    )
    .max(FILE_OP_MAX_ENTRIES),
  truncated: z.boolean(),
  filesScanned: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
});

export type ListFilesOptions = Omit<z.input<typeof fileListInputSchema>, 'path'>;
export type EditFileOptions = Omit<z.input<typeof fileEditInputSchema>, 'path'>;
export type SearchFilesOptions = Omit<z.input<typeof fileSearchInputSchema>, 'query' | 'path'> & { path?: string };

export function parseFileInput<T extends z.ZodType>(schema: T, value: unknown): z.output<T> {
  const parsed = schema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid structured file operation');
  return parsed.data;
}

export function parseFileOutput<T extends z.ZodType>(schema: T, value: unknown): z.output<T> {
  const parsed = schema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_response', 'invalid structured file operation response');
  return parsed.data;
}
