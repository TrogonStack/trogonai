import { create } from '@bufbuild/protobuf';
import { NodeIdSchema, type NodeId as NodeIdMessage } from '@trogonai/proto';

import { invalidProject } from './faults';

declare const nodeIdBrand: unique symbol;

export type NodeId = string & { readonly [nodeIdBrand]: true };

export function nodeIdProblem(value: string): string | undefined {
  if (value.length === 0) return 'A parent node is required.';
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return 'A node id is one identifier, never a path.';
  return undefined;
}

export function parseNodeId(value: string): NodeId | undefined {
  return nodeIdProblem(value) === undefined ? (value as NodeId) : undefined;
}

export function mintNodeId(value: string): NodeId {
  const problem = nodeIdProblem(value);
  if (problem) throw invalidProject(problem);
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
