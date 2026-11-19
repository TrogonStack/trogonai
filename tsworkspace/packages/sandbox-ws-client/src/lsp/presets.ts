export interface LspPreset {
  command: string;
  args: string[];
  languageId: string;
}

export const LSP_PRESETS = {
  typescript: { command: 'typescript-language-server', args: ['--stdio'], languageId: 'typescript' },
  python: { command: 'pyright-langserver', args: ['--stdio'], languageId: 'python' },
} as const satisfies Record<string, LspPreset>;

export type LspPresetName = keyof typeof LSP_PRESETS;
