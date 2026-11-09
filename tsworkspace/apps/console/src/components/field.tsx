import { ChevronDownIcon, CircleAlertIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Segmented, type SegmentedOption } from '@/components/ui/segmented';
import { cn } from '@/lib/utils';

export function describedBy(id: string, error: string | undefined, hint: string | undefined): string | undefined {
  const ids = [error ? `${id}-error` : undefined, hint ? `${id}-hint` : undefined].filter(Boolean);
  return ids.length > 0 ? ids.join(' ') : undefined;
}

export function firstMessage(errors: readonly unknown[]): string | undefined {
  for (const error of errors) {
    if (typeof error === 'string') return error;
    if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
      return error.message;
    }
  }
  return undefined;
}

export function FieldError({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <p id={id} role="alert" className="text-danger-ink flex items-start gap-1 text-xs">
      <CircleAlertIcon className="mt-px size-3.5 shrink-0" aria-hidden="true" />
      <span>{children}</span>
    </p>
  );
}

interface FieldShellProps {
  id: string;
  label: ReactNode;
  error?: string;
  hint?: string;
  className?: string;
  children: ReactNode;
}

export function FieldShell({ id, label, error, hint, className, children }: FieldShellProps) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-1.5', className)}>
      <Label htmlFor={id}>{label}</Label>
      {children}
      {hint ? (
        <p id={`${id}-hint`} className="text-muted-foreground text-xs">
          {hint}
        </p>
      ) : null}
      {error ? <FieldError id={`${id}-error`}>{error}</FieldError> : null}
    </div>
  );
}

interface TextFieldProps {
  id: string;
  label: ReactNode;
  value: string | number;
  onChange: (value: string) => void;
  onBlur?: () => void;
  error?: string;
  hint?: string;
  type?: 'text' | 'number';
  placeholder?: string;
  mono?: boolean;
  disabled?: boolean;
  className?: string;
  autoComplete?: string;
}

export function TextField({
  id,
  label,
  value,
  onChange,
  onBlur,
  error,
  hint,
  type = 'text',
  placeholder,
  mono,
  disabled,
  className,
  autoComplete = 'off',
}: TextFieldProps) {
  return (
    <FieldShell id={id} label={label} error={error} hint={hint} className={className}>
      <Input
        id={id}
        name={id}
        type={type}
        value={Number.isNaN(value) ? '' : value}
        placeholder={placeholder}
        disabled={disabled}
        autoComplete={autoComplete}
        spellCheck={false}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(id, error, hint)}
        className={cn('h-8', mono && 'font-mono text-xs md:text-xs', type === 'number' && 'tabular-nums')}
        onChange={(event) => onChange(event.target.value)}
        onBlur={onBlur}
      />
    </FieldShell>
  );
}

export interface SelectOption<T extends string | number> {
  readonly value: T;
  readonly label: string;
}

interface SelectFieldProps<T extends string | number> {
  id: string;
  label: string;
  value: T;
  options: readonly SelectOption<T>[];
  onChange: (value: T) => void;
  onBlur?: () => void;
  error?: string;
  hint?: string;
  disabled?: boolean;
  className?: string;
}

export function SelectField<T extends string | number>({
  id,
  label,
  value,
  options,
  onChange,
  onBlur,
  error,
  hint,
  disabled,
  className,
}: SelectFieldProps<T>) {
  return (
    <FieldShell id={id} label={label} error={error} hint={hint} className={className}>
      <NativeSelect
        id={id}
        value={value}
        options={options}
        onChange={onChange}
        onBlur={onBlur}
        disabled={disabled}
        invalid={Boolean(error)}
        describedBy={describedBy(id, error, hint)}
      />
    </FieldShell>
  );
}

interface NativeSelectProps<T extends string | number> {
  id?: string;
  value: T;
  options: readonly SelectOption<T>[];
  onChange: (value: T) => void;
  onBlur?: () => void;
  disabled?: boolean;
  invalid?: boolean;
  describedBy?: string;
  label?: string;
  className?: string;
}

export function NativeSelect<T extends string | number>({
  id,
  value,
  options,
  onChange,
  onBlur,
  disabled,
  invalid,
  describedBy: describedById,
  label,
  className,
}: NativeSelectProps<T>) {
  return (
    <div className={cn('relative w-full min-w-0', className)}>
      <select
        id={id}
        aria-label={label}
        value={value}
        disabled={disabled}
        aria-invalid={invalid ? true : undefined}
        aria-describedby={describedById}
        className="border-input bg-card focus-visible:border-ring focus-visible:ring-ring/60 aria-invalid:border-destructive dark:bg-input/30 h-8 w-full min-w-0 appearance-none rounded-md border py-0 pr-8 pl-2.5 text-[13px] shadow-xs transition-colors outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
        onChange={(event) => {
          const selected = options.find((option) => String(option.value) === event.target.value);
          if (selected) onChange(selected.value);
        }}
        onBlur={onBlur}
      >
        {options.map((option) => (
          <option key={String(option.value)} value={String(option.value)}>
            {option.label}
          </option>
        ))}
      </select>
      <ChevronDownIcon
        className="text-muted-foreground pointer-events-none absolute top-1/2 right-2.5 size-3.5 -translate-y-1/2"
        aria-hidden="true"
      />
    </div>
  );
}

interface SegmentedFieldProps<T extends string> {
  id: string;
  label: string;
  value: T;
  options: readonly SegmentedOption<T>[];
  onChange: (value: T) => void;
  hint?: string;
  error?: string;
  className?: string;
}

export function SegmentedField<T extends string>({
  id,
  label,
  value,
  options,
  onChange,
  hint,
  error,
  className,
}: SegmentedFieldProps<T>) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-1.5', className)}>
      <span id={`${id}-label`} className="micro text-muted-foreground">
        {label}
      </span>
      <Segmented
        label={label}
        value={value}
        options={options}
        onChange={onChange}
        className="w-fit max-w-full flex-wrap"
      />
      {hint ? <p className="text-muted-foreground text-xs">{hint}</p> : null}
      {error ? <FieldError id={`${id}-error`}>{error}</FieldError> : null}
    </div>
  );
}

export interface RadioCardOption<T extends string | number> {
  readonly value: T;
  readonly label: string;
  readonly description?: string;
}

interface RadioCardsProps<T extends string | number> {
  name: string;
  label: string;
  value: T;
  options: readonly RadioCardOption<T>[];
  onChange: (value: T) => void;
  hint?: string;
  error?: string;
  columns?: 1 | 2 | 3;
  className?: string;
}

const columnClasses = { 1: 'grid-cols-1', 2: 'grid-cols-1 sm:grid-cols-2', 3: 'grid-cols-1 sm:grid-cols-3' } as const;

export function RadioCards<T extends string | number>({
  name,
  label,
  value,
  options,
  onChange,
  hint,
  error,
  columns = 1,
  className,
}: RadioCardsProps<T>) {
  return (
    <fieldset className={cn('flex min-w-0 flex-col gap-1.5', className)}>
      <legend className="micro text-muted-foreground mb-1.5">{label}</legend>
      <div className={cn('grid gap-2', columnClasses[columns])}>
        {options.map((option) => {
          const inputId = `${name}-${String(option.value)}`;
          return (
            <label
              key={String(option.value)}
              htmlFor={inputId}
              className="group/radio border-input bg-card hover:bg-muted/60 has-checked:border-primary has-checked:bg-primary/8 has-focus-visible:ring-ring/70 relative flex cursor-pointer items-start gap-2.5 rounded-md border p-2.5 shadow-[inset_0_1px_0_0_var(--highlight)] transition-colors has-focus-visible:ring-2"
            >
              <input
                id={inputId}
                type="radio"
                name={name}
                className="peer sr-only"
                checked={option.value === value}
                onChange={() => onChange(option.value)}
              />
              <span
                aria-hidden="true"
                className="border-input bg-surface-deep group-has-checked/radio:border-primary mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border transition-colors"
              >
                <span className="led size-1.5 rounded-full opacity-0 transition-opacity [--led:var(--primary)] group-has-checked/radio:opacity-100" />
              </span>
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="text-[13px] leading-4 font-medium">{option.label}</span>
                {option.description ? (
                  <span className="text-muted-foreground text-xs">{option.description}</span>
                ) : null}
              </span>
            </label>
          );
        })}
      </div>
      {hint ? <p className="text-muted-foreground text-xs">{hint}</p> : null}
      {error ? <FieldError id={`${name}-error`}>{error}</FieldError> : null}
    </fieldset>
  );
}
