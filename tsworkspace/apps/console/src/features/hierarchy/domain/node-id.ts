import { create } from '@bufbuild/protobuf';
import { NodeIdSchema, type NodeId as NodeIdMessage } from '@trogonai/proto';

import { HierarchyFault } from './faults';

declare const nodeIdBrand: unique symbol;

export type NodeId = string & { readonly [nodeIdBrand]: true };

export const MAX_NODE_ID_LENGTH = 40;

const NEW_NODE_ID_PATTERN = /^[a-z][a-z0-9-]*$/;

const ROUTE_RESERVED_NODE_IDS: ReadonlySet<string> = new Set(['new']);

function reservedNodeIdProblem(value: string): string | undefined {
  return ROUTE_RESERVED_NODE_IDS.has(value)
    ? `A node id cannot be "${value}", because that address opens the create form.`
    : undefined;
}

export function nodeIdProblem(value: string): string | undefined {
  if (value.length === 0) return 'A parent node is required.';
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return 'A node id is one identifier, never a path.';
  return reservedNodeIdProblem(value);
}

export function parseNodeId(value: string): NodeId | undefined {
  return nodeIdProblem(value) === undefined ? (value as NodeId) : undefined;
}

export function mintNodeId(value: string): NodeId {
  const problem = nodeIdProblem(value);
  if (problem) throw new HierarchyFault('COMMAND_INVALID', problem);
  return value as NodeId;
}

export function nodeIdFromStored(value: string): NodeId {
  return value as NodeId;
}

export function nodeIdMessage(id: NodeId): NodeIdMessage {
  return create(NodeIdSchema, { value: id });
}

export function nodeIdOf(message: NodeIdMessage | undefined): NodeId {
  return nodeIdFromStored(message?.value ?? '');
}

export function newNodeIdProblem(value: string): string | undefined {
  if (value.length === 0) return 'An id is required.';
  if (value.length > MAX_NODE_ID_LENGTH) return `An id is at most ${MAX_NODE_ID_LENGTH} characters.`;
  if (!/^[a-z]/.test(value)) return 'An id starts with a lowercase letter.';
  if (!NEW_NODE_ID_PATTERN.test(value)) return 'An id uses only lowercase letters, digits and hyphens.';
  return reservedNodeIdProblem(value);
}

export function parseNewNodeId(value: string): NodeId | undefined {
  return newNodeIdProblem(value) === undefined ? (value as NodeId) : undefined;
}

export function suggestNodeId(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^[^a-z]+/, '')
    .slice(0, MAX_NODE_ID_LENGTH)
    .replace(/-+$/, '');
}
