import { CheckIcon, ChevronRightIcon, SearchIcon, XIcon } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent, MouseEvent } from 'react';

import { Kbd } from '@/components/ui/kbd';
import { Popover, PopoverContent } from '@/components/ui/popover';
import { cn } from '@/lib/utils';

import { emptyFilterState, facetCounts, isFiltering, removeFacet, setText, toggleValue } from './apply-filters';
import { FilterPill } from './filter-pill';
import { facetItems, suggestionItems, valueItems } from './suggestions';
import type { FilterItem } from './suggestions';
import type { FilterFacet, FilterState, TextMatcher } from './types';

interface FilterBarProps<Row> {
  rows: readonly Row[];
  facets: readonly FilterFacet<Row>[];
  state: FilterState;
  onChange: (state: FilterState) => void;
  matchesText?: TextMatcher<Row>;
  label: string;
  placeholder?: string;
  className?: string;
}

const MAX_INLINE_ICONS = 3;

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName);
}

function PillValue<Row>({ facet, selected }: { facet: FilterFacet<Row>; selected: readonly string[] }) {
  const options = selected.map(
    (value) => facet.values.find((option) => option.value === value) ?? { value, label: value },
  );
  if (options.length === 1) {
    const [only] = options;
    return (
      <>
        {only?.icon}
        {only?.label}
      </>
    );
  }
  return (
    <>
      <span className="flex items-center gap-1">
        {options.slice(0, MAX_INLINE_ICONS).map((option) => (
          <span key={option.value} className="flex items-center">
            {option.icon}
          </span>
        ))}
      </span>
      {options.length} values
    </>
  );
}

function ItemRow<Row>({
  id,
  item,
  active,
  grouped,
  onChoose,
  onHover,
}: {
  id: string;
  item: FilterItem<Row>;
  active: boolean;
  grouped: boolean;
  onChoose: () => void;
  onHover: () => void;
}) {
  const base = cn(
    'flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left text-[13px] text-foreground outline-none',
    active && 'bg-accent text-accent-foreground',
  );
  const handlers = {
    onMouseDown: (event: MouseEvent) => event.preventDefault(),
    onClick: onChoose,
    onMouseEnter: onHover,
  };

  if (item.kind === 'search') {
    return (
      <div id={id} role="option" aria-selected={active} className={base} {...handlers}>
        <SearchIcon aria-hidden="true" className="text-muted-foreground size-3.5" />
        <span className="min-w-0 truncate">
          Search for <span className="font-medium">&ldquo;{item.text}&rdquo;</span>
        </span>
      </div>
    );
  }

  if (item.kind === 'facet') {
    return (
      <div id={id} role="option" aria-selected={active} className={base} {...handlers}>
        <span className="text-muted-foreground flex items-center [&_svg]:size-3.5">{item.facet.icon}</span>
        <span className="min-w-0 flex-1 truncate">{item.facet.label}</span>
        <ChevronRightIcon aria-hidden="true" className="text-muted-foreground size-3.5" />
      </div>
    );
  }

  return (
    <div
      id={id}
      role="option"
      aria-selected={item.checked}
      aria-label={grouped ? `${item.facet.label}: ${item.option.label}` : item.option.label}
      className={cn(base, item.count === 0 && !item.checked && 'opacity-50')}
      {...handlers}
    >
      {grouped ? null : (
        <span
          aria-hidden="true"
          className={cn(
            'flex size-3.5 shrink-0 items-center justify-center rounded-[4px] border border-input',
            item.checked && 'border-primary bg-primary text-primary-foreground',
          )}
        >
          {item.checked ? <CheckIcon className="size-3" /> : null}
        </span>
      )}
      {grouped ? (
        <span className="text-muted-foreground flex shrink-0 items-center gap-1.5 [&_svg]:size-3.5">
          {item.facet.icon}
          {item.facet.label}:
        </span>
      ) : null}
      <span className="flex min-w-0 flex-1 items-center gap-2 [&_svg]:size-3.5">
        {item.option.icon}
        <span className="truncate">{item.option.label}</span>
      </span>
      <span className="text-muted-foreground font-mono text-xs tabular-nums">{item.count}</span>
    </div>
  );
}

export function FilterBar<Row>({
  rows,
  facets,
  state,
  onChange,
  matchesText,
  label,
  placeholder = 'Filter...',
  className,
}: FilterBarProps<Row>) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState(false);
  const [panelKey, setPanelKey] = useState<string | null>(null);
  const [panelQuery, setPanelQuery] = useState('');
  const [active, setActive] = useState(0);

  const counts = useMemo(() => facetCounts(rows, state, facets, matchesText), [rows, state, facets, matchesText]);
  const panelFacet = panelKey === null ? undefined : facets.find((facet) => facet.key === panelKey);

  const items = useMemo<FilterItem<Row>[]>(() => {
    if (panelFacet) return valueItems(panelFacet, panelQuery, counts, state);
    return query.trim() === '' ? facetItems(facets) : suggestionItems(query, facets, counts, state);
  }, [panelFacet, panelQuery, query, facets, counts, state]);

  const activeIndex = Math.min(active, Math.max(items.length - 1, 0));
  const activeItem = items[activeIndex];
  const activeId = open && activeItem ? `${listId}-${activeItem.id}` : undefined;
  const grouped = !panelFacet && query.trim() !== '';

  useEffect(() => {
    if (activeId) document.getElementById(activeId)?.scrollIntoView?.({ block: 'nearest' });
  }, [activeId]);

  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key.toLowerCase() !== 'f' || event.metaKey || event.ctrlKey || event.altKey) return;
      if (isTypingTarget(event.target)) return;
      event.preventDefault();
      inputRef.current?.focus();
      setOpen(true);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  const skipOpenOnFocus = useRef(false);
  const panelOrigin = useRef<FilterState | null>(null);

  const focusInput = () => {
    skipOpenOnFocus.current = true;
    inputRef.current?.focus();
    skipOpenOnFocus.current = false;
  };

  const closeAll = () => {
    panelOrigin.current = null;
    setOpen(false);
    setPanelKey(null);
    setPanelQuery('');
    setActive(0);
  };

  const openPanel = (facetKey: string) => {
    if (panelKey !== facetKey) panelOrigin.current = state;
    setPanelKey(facetKey);
    setPanelQuery('');
    setActive(0);
    setOpen(true);
  };

  const leavePanel = () => {
    panelOrigin.current = null;
    setPanelKey(null);
    setPanelQuery('');
    setActive(0);
    focusInput();
  };

  const cancelPanel = () => {
    if (panelOrigin.current) onChange(panelOrigin.current);
    leavePanel();
  };

  const choose = (item: FilterItem<Row>) => {
    if (item.kind === 'search') {
      onChange(setText(state, item.text));
      setQuery('');
      closeAll();
      focusInput();
      return;
    }
    if (item.kind === 'facet') {
      openPanel(item.facet.key);
      return;
    }
    onChange(toggleValue(state, item.facet.key, item.option.value));
    if (!panelFacet) {
      setQuery('');
      closeAll();
      focusInput();
    }
  };

  const removeLastPill = () => {
    if (state.text !== '') {
      onChange(setText(state, ''));
      return;
    }
    const keys = Object.keys(state.selected);
    const last = keys.at(-1);
    if (last !== undefined) onChange(removeFacet(state, last));
  };

  const move = (delta: number) => {
    if (items.length === 0) return;
    setActive((activeIndex + delta + items.length) % items.length);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>, inPanel: boolean) => {
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp':
        event.preventDefault();
        if (!open) {
          setOpen(true);
          return;
        }
        move(event.key === 'ArrowDown' ? 1 : -1);
        return;
      case 'Enter':
        if (open && activeItem) {
          event.preventDefault();
          choose(activeItem);
        }
        return;
      case 'Escape':
        if (inPanel) {
          event.preventDefault();
          event.stopPropagation();
          cancelPanel();
          return;
        }
        if (open) {
          event.preventDefault();
          event.stopPropagation();
          closeAll();
          focusInput();
        }
        return;
      case 'Tab':
        closeAll();
        return;
      case 'Backspace': {
        const empty = event.currentTarget.value === '';
        if (!empty) return;
        if (inPanel) {
          event.preventDefault();
          leavePanel();
          return;
        }
        removeLastPill();
        return;
      }
      default:
    }
  };

  const hasPills = state.text !== '' || Object.keys(state.selected).length > 0;
  const activeFacets = facets.filter((facet) => (state.selected[facet.key]?.length ?? 0) > 0);

  return (
    <div ref={rootRef} data-slot="filter-bar" className={cn('min-w-0', className)}>
      <div
        ref={boxRef}
        onClick={(event) => {
          if (event.target === event.currentTarget) focusInput();
        }}
        className="border-input bg-card focus-within:border-ring focus-within:ring-ring/50 dark:bg-input/30 flex min-h-9 cursor-text flex-wrap items-center gap-1.5 rounded-lg border px-2 py-1 shadow-xs transition-colors focus-within:ring-3"
      >
        <SearchIcon aria-hidden="true" className="text-muted-foreground size-3.5 shrink-0" />
        {activeFacets.map((facet) => (
          <FilterPill
            key={facet.key}
            icon={facet.icon}
            label={facet.label}
            operator={(state.selected[facet.key]?.length ?? 0) > 1 ? 'is any of' : 'is'}
            value={<PillValue facet={facet} selected={state.selected[facet.key] ?? []} />}
            onEdit={() => openPanel(facet.key)}
            onRemove={() => {
              onChange(removeFacet(state, facet.key));
              focusInput();
            }}
          />
        ))}
        {state.text !== '' ? (
          <FilterPill
            icon={<SearchIcon aria-hidden="true" />}
            label="Search"
            operator="contains"
            value={<span className="max-w-48 truncate">&ldquo;{state.text}&rdquo;</span>}
            onRemove={() => {
              onChange(setText(state, ''));
              focusInput();
            }}
          />
        ) : null}
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-label={label}
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-activedescendant={panelFacet ? undefined : activeId}
          aria-autocomplete="list"
          autoComplete="off"
          spellCheck={false}
          value={query}
          placeholder={hasPills ? '' : placeholder}
          onChange={(event) => {
            setQuery(event.target.value);
            setPanelKey(null);
            setActive(0);
            setOpen(true);
          }}
          onFocus={() => {
            if (!skipOpenOnFocus.current) setOpen(true);
          }}
          onClick={() => setOpen(true)}
          onKeyDown={(event) => onKeyDown(event, false)}
          className="placeholder:text-muted-foreground/45 h-6 min-w-24 flex-1 bg-transparent text-[13px] outline-none"
        />
        {isFiltering(state) ? (
          <button
            type="button"
            aria-label="Clear all filters"
            onClick={() => {
              onChange(emptyFilterState);
              setQuery('');
              closeAll();
              focusInput();
            }}
            className="text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/50 flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md outline-none focus-visible:ring-2"
          >
            <XIcon aria-hidden="true" className="size-3.5" />
          </button>
        ) : null}
      </div>
      <Popover
        open={open}
        onOpenChange={(next, details) => {
          if (next) return;
          const target = details.event?.target;
          if (target instanceof Node && rootRef.current?.contains(target)) return;
          closeAll();
        }}
      >
        <PopoverContent anchor={boxRef} initialFocus={false} finalFocus={false} className="w-80">
          {panelFacet ? (
            <div className="border-border flex items-center gap-2 border-b px-2.5 py-1.5">
              <span className="text-muted-foreground flex items-center gap-1.5 text-xs [&_svg]:size-3.5">
                {panelFacet.icon}
                {panelFacet.label}
              </span>
              <input
                autoFocus
                type="text"
                role="combobox"
                aria-label={`Filter ${panelFacet.label}`}
                aria-expanded
                aria-controls={listId}
                aria-activedescendant={activeId}
                aria-autocomplete="list"
                autoComplete="off"
                spellCheck={false}
                value={panelQuery}
                placeholder="Filter..."
                onChange={(event) => {
                  setPanelQuery(event.target.value);
                  setActive(0);
                }}
                onKeyDown={(event) => onKeyDown(event, true)}
                className="placeholder:text-muted-foreground/45 h-6 min-w-0 flex-1 bg-transparent text-[13px] outline-none"
              />
            </div>
          ) : (
            <div className="border-border flex items-center justify-between gap-2 border-b px-2.5 py-1.5">
              <span className="micro text-muted-foreground text-[10px]">
                {query.trim() === '' ? 'Add filter' : 'Suggestions'}
              </span>
              <Kbd>F</Kbd>
            </div>
          )}
          <div
            id={listId}
            role="listbox"
            aria-label={panelFacet ? `${panelFacet.label} values` : 'Filter suggestions'}
            aria-multiselectable={panelFacet ? true : undefined}
            className="max-h-72 overflow-y-auto p-1"
          >
            {items.length === 0 ? (
              <p className="text-muted-foreground px-2 py-3 text-center text-xs">No matches</p>
            ) : (
              items.map((item, index) => (
                <ItemRow
                  key={item.id}
                  id={`${listId}-${item.id}`}
                  item={item}
                  active={index === activeIndex}
                  grouped={grouped}
                  onChoose={() => choose(item)}
                  onHover={() => setActive(index)}
                />
              ))
            )}
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
