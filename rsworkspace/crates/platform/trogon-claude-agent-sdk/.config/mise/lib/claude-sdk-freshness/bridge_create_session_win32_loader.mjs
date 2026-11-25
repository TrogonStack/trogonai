// Module hooks for Windows scenarios. Only imports made by the bundle named in WIN32_PATH_PARENT are redirected.
const SHIM = 'win32-path-shim:///path';
const NAMES = ['basename', 'dirname', 'extname', 'format', 'isAbsolute', 'join', 'normalize', 'parse', 'relative', 'resolve', 'sep', 'delimiter', 'toNamespacedPath'];

export async function resolve(specifier, context, next) {
  if ((specifier === 'node:path' || specifier === 'path') && context.parentURL === process.env.WIN32_PATH_PARENT) {
    return { url: SHIM, shortCircuit: true };
  }
  return next(specifier, context);
}

export async function load(url, context, next) {
  if (url !== SHIM) return next(url, context);
  const source = [
    "import flavor from 'node:path/win32';",
    `export const { ${NAMES.join(', ')} } = flavor;`,
    'export const win32 = flavor;',
    'export const posix = flavor.posix;',
    'export default flavor;',
  ].join('\n');
  return { format: 'module', source, shortCircuit: true };
}
