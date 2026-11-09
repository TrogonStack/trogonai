import { useEffect, useState } from 'react';

export function useNow(intervalMs = 1000): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

function span(milliseconds: number): string {
  const seconds = Math.floor(Math.abs(milliseconds) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

export function relativeTime(date: Date, now: Date = new Date()): string {
  const delta = date.getTime() - now.getTime();
  if (Math.abs(delta) < 45_000) return 'just now';
  return delta > 0 ? `in ${span(delta)}` : `${span(delta)} ago`;
}

export function countdown(until: Date, now: Date): string {
  const remaining = Math.max(0, until.getTime() - now.getTime());
  const totalSeconds = Math.floor(remaining / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;
}

export function formatTimestamp(date: Date): string {
  return date.toLocaleString([], { hour12: false });
}

export function formatCompact(date: Date): string {
  return date.toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false });
}
