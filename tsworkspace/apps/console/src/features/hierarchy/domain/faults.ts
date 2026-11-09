export type HierarchyFaultReason =
  | 'COMMAND_INVALID'
  | 'NODE_ALREADY_EXISTS'
  | 'NODE_NOT_FOUND'
  | 'PARENT_NOT_FOUND'
  | 'NODE_CYCLE'
  | 'DEPTH_EXCEEDED'
  | 'ROOT_IMMUTABLE'
  | 'NODE_HAS_CHILDREN'
  | 'NODE_HAS_ATTACHMENTS'
  | 'REVISION_CONFLICT';

const messages: Record<HierarchyFaultReason, string> = {
  COMMAND_INVALID: 'The command is malformed.',
  NODE_ALREADY_EXISTS: 'A node with this id already exists.',
  NODE_NOT_FOUND: 'The node does not exist.',
  PARENT_NOT_FOUND: 'The parent node does not exist.',
  NODE_CYCLE: 'A node cannot be moved under itself or under one of its own descendants.',
  DEPTH_EXCEEDED: 'The change would nest nodes deeper than the tree allows.',
  ROOT_IMMUTABLE: 'The root can be neither moved nor removed.',
  NODE_HAS_CHILDREN: 'The node still has child nodes.',
  NODE_HAS_ATTACHMENTS: 'The node still has resources attached to it.',
  REVISION_CONFLICT: 'The tree changed since it was loaded. Reload and try again.',
};

export class HierarchyFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: HierarchyFaultReason,
    readonly detail?: string,
  ) {
    super(detail ? `${messages[reason]} ${detail}` : messages[reason]);
    this.name = 'HierarchyFault';
  }
}

export function isHierarchyFault(error: unknown): error is HierarchyFault {
  return error instanceof HierarchyFault;
}
