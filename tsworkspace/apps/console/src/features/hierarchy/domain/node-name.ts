import { HierarchyFault } from './faults';

declare const nodeNameBrand: unique symbol;

export type NodeName = string & { readonly [nodeNameBrand]: true };

export const MAX_NODE_NAME_LENGTH = 60;

export function nodeNameProblem(value: string): string | undefined {
  const name = value.trim();
  if (name.length === 0) return 'A name is required.';
  if (name.length > MAX_NODE_NAME_LENGTH) return `A name is at most ${MAX_NODE_NAME_LENGTH} characters.`;
  return undefined;
}

export function parseNodeName(value: string): NodeName | undefined {
  return nodeNameProblem(value) === undefined ? (value.trim() as NodeName) : undefined;
}

export function mintNodeName(value: string): NodeName {
  const problem = nodeNameProblem(value);
  if (problem) throw new HierarchyFault('COMMAND_INVALID', problem);
  return value.trim() as NodeName;
}
