import { CalendarPlusIcon, PencilIcon, PlusIcon, SearchIcon, Trash2Icon } from 'lucide-react';
import { useState, type KeyboardEvent } from 'react';

import { FieldError, FieldShell } from '@/components/field';
import { Button } from '@/components/ui/button';
import { DateTimePicker } from '@/components/ui/date-time-picker';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';

import {
  compareExceptionRows,
  exactInstant,
  exceptionLines,
  friendlyInstant,
  importExceptionDates,
  reconcileExceptionRows,
  type ExceptionDateRow,
} from '../exception-dates';

function pickerText(value: string): string {
  const instant = exactInstant(value);
  return instant ? new Date(instant.iso).toISOString().slice(0, 19) : '';
}
function pickerInstant(value: string): string {
  return value ? `${value.length === 16 ? `${value}:00` : value}Z` : '';
}

export interface ExceptionDatesProps {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  opposingValue?: string;
  error?: string;
}
export function ExceptionDates({ id, label, value, onChange, opposingValue = '', error }: ExceptionDatesProps) {
  const [current, setCurrent] = useState(() => ({ value, rows: reconcileExceptionRows(value) }));
  if (current.value !== value) setCurrent({ value, rows: reconcileExceptionRows(value, current.rows) });
  const rows = current.rows;
  const included = id === 'rdate';
  const singular = included ? 'included date' : 'excluded date';
  const [date, setDate] = useState('');
  const [precise, setPrecise] = useState(false);
  const [editing, setEditing] = useState<{ id: string; value: string; precise: boolean } | null>(null);
  const [feedback, setFeedback] = useState('');
  const [inputError, setInputError] = useState('');
  const [editError, setEditError] = useState('');
  const [batch, setBatch] = useState('');
  const [batchError, setBatchError] = useState('');
  const [search, setSearch] = useState('');
  const [clearing, setClearing] = useState(false);
  const opposingKeys = new Set(
    exceptionLines(opposingValue)
      .map((raw) => exactInstant(raw)?.key)
      .filter(Boolean),
  );
  const duplicateCounts = new Map<string, number>();
  for (const row of rows) {
    const key = exactInstant(row.raw)?.key;
    if (key) duplicateCounts.set(key, (duplicateCounts.get(key) ?? 0) + 1);
  }
  const sorted = [...rows].sort(compareExceptionRows).filter((row) => {
    if (!search) return true;
    const instant = exactInstant(row.raw);
    const friendly = instant ? friendlyInstant(instant) : undefined;
    return `${row.raw} ${friendly?.date ?? ''} ${friendly?.time ?? ''}`.toLowerCase().includes(search.toLowerCase());
  });
  const publish = (next: ExceptionDateRow[]) => {
    const nextValue = next.map((row) => row.raw).join('\n');
    setCurrent({ value: nextValue, rows: next });
    onChange(nextValue);
    setClearing(false);
  };
  const add = () => {
    const raw = date.trim();
    const instant = exactInstant(raw);
    if (!instant) {
      setInputError('Choose a valid UTC date or enter an ISO instant with an offset (up to nine fractional digits).');
      return;
    }
    if (rows.some((row) => exactInstant(row.raw)?.key === instant.key)) {
      setInputError('This instant is already in the list, even if its timezone or formatting differs.');
      return;
    }
    publish([...rows, { id: crypto.randomUUID(), raw }]);
    setDate('');
    setInputError('');
    setFeedback('Date added.');
  };
  const save = () => {
    if (!editing) return;
    if (!rows.some((row) => row.id === editing.id)) {
      setEditing(null);
      return;
    }
    const raw = editing.value.trim();
    const instant = exactInstant(raw);
    if (!instant) {
      setEditError('Enter a valid ISO instant with Z or an offset, with up to nine fractional digits.');
      return;
    }
    if (rows.some((row) => row.id !== editing.id && exactInstant(row.raw)?.key === instant.key)) {
      setEditError('This instant is already in the list.');
      return;
    }
    publish(rows.map((row) => (row.id === editing.id ? { ...row, raw } : row)));
    setEditing(null);
    setEditError('');
    setFeedback('Date updated.');
  };
  const importDates = () => {
    const result = importExceptionDates(batch, rows);
    if (!result.ok) {
      setBatchError(
        `Invalid ISO dates on line${result.invalidLines.length === 1 ? '' : 's'} ${result.invalidLines.join(', ')}. Nothing was imported.`,
      );
      return;
    }
    if (result.added.length) publish([...rows, ...result.added.map((raw) => ({ id: crypto.randomUUID(), raw }))]);
    setBatchError('');
    setBatch('');
    setFeedback(
      `Imported ${result.added.length} date${result.added.length === 1 ? '' : 's'}; skipped ${result.duplicates} duplicate${result.duplicates === 1 ? '' : 's'}.`,
    );
  };
  const containEnter = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && event.target instanceof HTMLInputElement) event.preventDefault();
  };
  return (
    <section
      aria-label={label}
      className="border-border bg-card flex min-w-0 flex-col rounded-lg border"
      onKeyDown={containEnter}
    >
      <div className="border-border flex items-start justify-between gap-2 border-b p-3">
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-medium">{label}</h3>
            <span className="bg-muted text-muted-foreground rounded px-1.5 py-0.5 font-mono text-[10px]">
              {rows.length}
            </span>
          </div>
          <p className="text-muted-foreground text-xs">
            {included ? 'Extra occurrences beyond the recurrence rule.' : 'Occurrences to skip, including extra dates.'}
          </p>
        </div>
        {rows.length ? (
          <Button type="button" variant="ghost" size="sm" onClick={() => setClearing(true)}>
            Clear all
          </Button>
        ) : null}
      </div>
      {clearing ? (
        <div
          role="group"
          aria-label={`Clear ${label.toLowerCase()} confirmation`}
          className="bg-warning-soft flex flex-wrap items-center gap-2 px-3 py-2 text-xs"
        >
          <span>Remove all {rows.length} dates?</span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              publish([]);
              setEditing(null);
              setFeedback('All dates removed.');
            }}
          >
            Confirm clear
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => setClearing(false)}>
            Keep dates
          </Button>
        </div>
      ) : null}
      {rows.length > 5 || search ? (
        <div className="flex items-center gap-2 px-3 pt-3">
          <SearchIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" />
          <Input
            aria-label={`Search ${label.toLowerCase()}`}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Find a date…"
            className="h-7"
          />
        </div>
      ) : null}
      {!rows.length ? (
        <div className="flex flex-col items-center gap-1 px-4 py-6 text-center">
          <CalendarPlusIcon className="text-muted-foreground/60 mb-1 size-5" aria-hidden="true" />
          <p className="text-muted-foreground text-xs">No {label.toLowerCase()} yet</p>
        </div>
      ) : (
        <ul
          aria-label={`${label} entries`}
          className="divide-border flex max-h-72 flex-col divide-y overflow-y-auto px-3"
        >
          {sorted.map((row) => {
            const instant = exactInstant(row.raw);
            const friendly = instant ? friendlyInstant(instant) : undefined;
            const conflict = instant && opposingKeys.has(instant.key);
            const duplicate = instant && (duplicateCounts.get(instant.key) ?? 0) > 1;
            return (
              <li key={row.id} className="flex min-w-0 flex-col gap-2 py-2.5">
                {editing?.id === row.id ? (
                  <div
                    className="flex flex-col gap-2"
                    onKeyDown={(event) => {
                      if (
                        event.key === 'Escape' &&
                        !(event.target instanceof Element && event.target.closest('[data-slot="popover-content"]'))
                      ) {
                        event.preventDefault();
                        setEditing(null);
                        setEditError('');
                      }
                    }}
                  >
                    <FieldShell
                      id={`${id}-edit-${row.id}`}
                      label={`Edit ${singular}`}
                      error={editError}
                      hint={
                        editing.precise
                          ? 'ISO instant with Z or an offset. The original precision is retained until you save.'
                          : 'Choose the instant in UTC. Switch to precise ISO to inspect fractional seconds or an offset.'
                      }
                    >
                      {editing.precise ? (
                        <Input
                          id={`${id}-edit-${row.id}`}
                          type="text"
                          value={editing.value}
                          onChange={(event) => {
                            setEditing({
                              ...editing,
                              value: event.target.value,
                            });
                            setEditError('');
                          }}
                          onKeyDown={(event) => {
                            if (event.key === 'Escape') {
                              event.preventDefault();
                              setEditing(null);
                              setEditError('');
                            }
                            if (event.key === 'Enter') {
                              event.preventDefault();
                              save();
                            }
                          }}
                          autoFocus
                          className="font-mono text-xs"
                          aria-invalid={Boolean(editError)}
                          aria-describedby={editError ? `${id}-edit-${row.id}-error` : `${id}-edit-${row.id}-hint`}
                        />
                      ) : (
                        <DateTimePicker
                          autoFocus
                          id={`${id}-edit-${row.id}`}
                          label={`Edit ${singular}`}
                          value={pickerText(editing.value)}
                          onChange={(fields) => {
                            setEditing({ ...editing, value: pickerInstant(fields) });
                            setEditError('');
                          }}
                          zoneLabel="UTC"
                          invalid={Boolean(editError)}
                          describedBy={editError ? `${id}-edit-${row.id}-error` : `${id}-edit-${row.id}-hint`}
                        />
                      )}
                    </FieldShell>
                    <button
                      type="button"
                      className="text-muted-foreground hover:text-foreground w-fit text-[11px] underline underline-offset-2"
                      onClick={() => setEditing({ ...editing, precise: !editing.precise })}
                    >
                      {editing.precise
                        ? `Use UTC picker for editing ${singular}`
                        : `Use precise ISO for editing ${singular}`}
                    </button>
                    <div className="flex gap-2">
                      <Button type="button" size="sm" onClick={save}>
                        Save date
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          setEditing(null);
                          setEditError('');
                        }}
                      >
                        Cancel edit
                      </Button>
                    </div>
                  </div>
                ) : (
                  <div className="flex min-w-0 items-start justify-between gap-2">
                    <div className="flex min-w-0 flex-col gap-0.5">
                      {friendly ? (
                        <>
                          <span className="text-xs font-medium">{friendly.date}</span>
                          <span className="text-muted-foreground font-mono text-[11px]">{friendly.time}</span>
                        </>
                      ) : (
                        <span className="text-danger-ink text-xs font-medium">Invalid date</span>
                      )}
                      <span title={row.raw} className="text-muted-foreground/75 truncate font-mono text-[10px]">
                        {row.raw}
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-0.5">
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Edit ${row.raw.trim()}`}
                        onClick={() => {
                          setEditing({
                            id: row.id,
                            value: row.raw,
                            precise: !exactInstant(row.raw) || !row.raw.endsWith('Z') || /\.\d+/.test(row.raw),
                          });
                          setEditError('');
                        }}
                      >
                        <PencilIcon className="size-3.5" />
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Remove ${row.raw.trim()}`}
                        onClick={() => {
                          publish(rows.filter((entry) => entry.id !== row.id));
                          setFeedback('Date removed.');
                        }}
                      >
                        <Trash2Icon className="size-3.5" />
                      </Button>
                    </div>
                  </div>
                )}
                {!instant ? (
                  <p className="text-danger-ink text-[11px]">Fix or remove this entry before creating the schedule.</p>
                ) : null}
                {duplicate ? <p className="text-warning-ink text-[11px]">Duplicate instant in this list.</p> : null}
                {conflict ? (
                  <p className="text-warning-ink text-[11px]">
                    {included
                      ? 'Also excluded. This occurrence will be skipped.'
                      : 'Also included. Exclusion takes precedence.'}
                  </p>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
      {rows.length > 0 && !sorted.length ? (
        <p className="text-muted-foreground px-3 py-4 text-center text-xs">No matching dates.</p>
      ) : null}
      <div className="border-border flex flex-col gap-2 border-t p-3">
        <FieldShell
          id={`${id}-add`}
          label={precise ? `New ${singular} (ISO)` : `New ${singular} (UTC)`}
          error={inputError}
        >
          <div className="flex items-center gap-2">
            {precise ? (
              <Input
                id={`${id}-add`}
                type="text"
                value={date}
                placeholder="2027-01-15T09:00:00Z"
                onChange={(event) => {
                  setDate(event.target.value);
                  setInputError('');
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    add();
                  }
                }}
                aria-invalid={Boolean(inputError)}
                aria-describedby={inputError ? `${id}-add-error` : undefined}
                className={precise ? 'font-mono text-xs' : ''}
              />
            ) : (
              <DateTimePicker
                id={`${id}-add`}
                label={`New ${singular} (UTC)`}
                value={pickerText(date)}
                onChange={(fields) => {
                  setDate(pickerInstant(fields));
                  setInputError('');
                }}
                zoneLabel="UTC"
                invalid={Boolean(inputError)}
                describedBy={inputError ? `${id}-add-error` : undefined}
              />
            )}
            <Button
              type="button"
              variant="outline"
              size="icon"
              aria-label={`Add ${singular}`}
              onClick={add}
              disabled={!date}
            >
              <PlusIcon />
            </Button>
          </div>
        </FieldShell>
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground w-fit text-[11px] underline underline-offset-2"
          onClick={() => {
            setPrecise(!precise);
            setInputError('');
          }}
        >
          {precise ? `Use UTC picker for ${label.toLowerCase()}` : `Use precise ISO for ${label.toLowerCase()}`}
        </button>
      </div>
      <details className="border-border border-t px-3 py-2">
        <summary className="text-muted-foreground cursor-pointer text-xs">Import dates</summary>
        <div className="flex flex-col gap-2 pt-3">
          <FieldShell
            id={`${id}-import`}
            label={`Paste ${label.toLowerCase()}`}
            hint="One ISO instant per line. The entire batch is checked before adding; duplicates are skipped."
            error={batchError}
          >
            <Textarea
              id={`${id}-import`}
              value={batch}
              onChange={(event) => {
                setBatch(event.target.value);
                setBatchError('');
              }}
              onKeyDown={(event) => event.stopPropagation()}
              className="min-h-24 font-mono text-xs"
            />
          </FieldShell>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            disabled={!batch.trim()}
            onClick={importDates}
          >
            Import dates
          </Button>
        </div>
      </details>
      {feedback ? (
        <p role="status" className="text-muted-foreground border-border border-t px-3 py-2 text-xs">
          {feedback}
        </p>
      ) : null}
      {error ? (
        <div className="p-3">
          <FieldError>{error}</FieldError>
        </div>
      ) : null}
    </section>
  );
}
