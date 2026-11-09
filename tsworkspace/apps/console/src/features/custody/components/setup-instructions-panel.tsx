import { CopyButton } from '@/components/copy-button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

import type { SetupInstruction } from '../domain/validation';

const languageLabels: Record<SetupInstruction['language'], string> = { json: 'JSON', bash: 'Shell', hcl: 'HCL' };

export function SetupInstructionsPanel({ instructions }: { instructions: readonly SetupInstruction[] }) {
  return (
    <div className="flex flex-col gap-4">
      {instructions.map((instruction) => (
        <Card key={instruction.id}>
          <CardHeader>
            <div className="flex flex-col gap-1">
              <CardTitle>{instruction.title}</CardTitle>
              <CardDescription>{instruction.description}</CardDescription>
            </div>
            <span className="micro text-muted-foreground text-[10px]">{languageLabels[instruction.language]}</span>
          </CardHeader>
          <CardContent className="relative">
            <pre
              tabIndex={0}
              aria-label={instruction.title}
              className="bg-surface-deep border-border overflow-x-auto rounded-md border p-3 pr-10 font-mono text-xs leading-5"
            >
              <code>{instruction.content}</code>
            </pre>
            <div className="absolute top-6 right-7">
              <CopyButton value={instruction.content} label={instruction.title.toLowerCase()} />
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
