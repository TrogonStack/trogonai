import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from '@tanstack/react-table';
import { ArrowDownIcon, ArrowUpIcon, ChevronRightIcon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { Skeleton } from '@/components/ui/skeleton';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { cn } from '@/lib/utils';

const ACTION_COLUMN = 'open';

interface DataTableProps<Row> {
  data: readonly Row[];
  columns: ColumnDef<Row, any>[];
  toolbar: ReactNode;
  empty: ReactNode;
  footer: ReactNode;
  onOpen: (row: Row) => void;
  muted?: (row: Row) => boolean;
}

export function DataTable<Row>({ data, columns, toolbar, empty, footer, onOpen, muted }: DataTableProps<Row>) {
  const [sorting, setSorting] = useState<SortingState>([]);
  const table = useReactTable({
    data: data as Row[],
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });
  const rows = table.getRowModel().rows;

  return (
    <div className="border-border bg-card min-w-0 overflow-hidden rounded-lg border shadow-[inset_0_1px_0_0_var(--highlight)]">
      <div className="border-border border-b p-3">{toolbar}</div>
      {rows.length === 0 ? (
        empty
      ) : (
        <Table>
          <TableHeader>
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id} className="hover:bg-transparent">
                {group.headers.map((header) => {
                  const sorted = header.column.getIsSorted();
                  const sortable = header.column.getCanSort() && header.column.id !== ACTION_COLUMN;
                  return (
                    <TableHead
                      key={header.id}
                      aria-sort={sorted === 'asc' ? 'ascending' : sorted === 'desc' ? 'descending' : 'none'}
                    >
                      {sortable ? (
                        <button
                          type="button"
                          className="hover:text-foreground focus-visible:ring-ring/60 flex items-center gap-1 rounded-sm text-left uppercase outline-none focus-visible:ring-2"
                          onClick={header.column.getToggleSortingHandler()}
                        >
                          {flexRender(header.column.columnDef.header, header.getContext())}
                          {sorted === 'asc' ? <ArrowUpIcon className="size-3" aria-hidden="true" /> : null}
                          {sorted === 'desc' ? <ArrowDownIcon className="size-3" aria-hidden="true" /> : null}
                        </button>
                      ) : (
                        flexRender(header.column.columnDef.header, header.getContext())
                      )}
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <TableRow
                key={row.id}
                className={cn('group/row cursor-pointer', muted?.(row.original) && 'text-muted-foreground opacity-70')}
                onClick={(event) => {
                  if ((event.target as HTMLElement).closest('a,button')) return;
                  onOpen(row.original);
                }}
              >
                {row.getVisibleCells().map((cell) => (
                  <TableCell key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {footer}
    </div>
  );
}

export function RowChevron() {
  return (
    <ChevronRightIcon
      className="text-muted-foreground/50 group-hover/row:text-muted-foreground size-4 transition-transform group-hover/row:translate-x-0.5"
      aria-hidden="true"
    />
  );
}

export function ActionColumnHeader() {
  return <span className="sr-only">Open</span>;
}

const dither = [
  '  .  :  .  :  .  :  .  ',
  ' :  .  : +++ :  .  :  . ',
  '  .  : +   + :  .  :  .  ',
  ' :  .  : +++ :  .  :  . ',
  '  .  :  .  :  .  :  .  ',
];

export function TableEmptyState({
  title,
  message,
  children,
}: {
  title: string;
  message: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center gap-4 px-6 py-14 text-center">
      <pre
        aria-hidden="true"
        className="text-muted-foreground/60 font-mono text-[11px] leading-3.5 whitespace-pre select-none"
      >
        {dither.join('\n')}
      </pre>
      <div className="flex flex-col gap-1.5">
        <p className="micro text-foreground text-xs">{title}</p>
        <p className="text-muted-foreground max-w-sm text-[13px]">{message}</p>
      </div>
      {children}
    </div>
  );
}

export function TableFooter({ shown, total, noun }: { shown: number; total: number; noun: string }) {
  return (
    <div className="micro border-border bg-surface-deep/50 text-muted-foreground flex items-center justify-between border-t px-3 py-2 text-[10px]">
      <span className="tabular-nums">
        {shown} of {total} {noun}
        {total === 1 ? '' : 's'}
      </span>
    </div>
  );
}

export function TableSkeleton({ label, leading }: { label: string; leading?: boolean }) {
  return (
    <div
      role="status"
      aria-busy="true"
      className="border-border bg-card overflow-hidden rounded-lg border shadow-[inset_0_1px_0_0_var(--highlight)]"
    >
      <span className="sr-only">{label}</span>
      <div className="border-border border-b p-3">
        <Skeleton className="h-9 w-full" />
      </div>
      <div className="flex flex-col">
        {Array.from({ length: 5 }, (_, index) => (
          <div key={index} className="border-border flex items-center gap-4 border-b px-3 py-3 last:border-0">
            {leading ? <Skeleton className="size-8" /> : null}
            <div className="flex flex-1 flex-col gap-1.5">
              <Skeleton className="h-3.5 w-40" />
              <Skeleton className="h-3 w-24" />
            </div>
            <Skeleton className="h-5 w-16" />
            <Skeleton className="h-5 w-20" />
          </div>
        ))}
      </div>
    </div>
  );
}
