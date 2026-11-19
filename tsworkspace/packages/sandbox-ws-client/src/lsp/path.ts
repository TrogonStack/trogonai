export function isAbsolutePosix(path: string): boolean {
  return path.startsWith('/');
}

/** Resolves `path` against the absolute `base`, collapsing `.`, `..` and duplicate slashes. */
export function resolvePosix(base: string, path: string): string {
  if (!isAbsolutePosix(base)) {
    throw new Error(`base must be an absolute path, got "${base}"`);
  }
  const segments: string[] = [];
  for (const segment of (isAbsolutePosix(path) ? path : `${base}/${path}`).split('/')) {
    if (segment === '' || segment === '.') {
      continue;
    }
    if (segment === '..') {
      segments.pop();
    } else {
      segments.push(segment);
    }
  }
  return `/${segments.join('/')}`;
}

export function basenamePosix(path: string): string {
  const segments = path.split('/').filter((segment) => segment !== '');
  return segments[segments.length - 1] ?? '';
}

export function fileUri(absolutePath: string): string {
  return `file://${absolutePath.split('/').map(encodeURIComponent).join('/')}`;
}
