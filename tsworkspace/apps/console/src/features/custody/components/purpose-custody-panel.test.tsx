import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import type { CustodyView } from '../domain/view';
import { PurposeCustodyPanel } from './purpose-custody-panel';

const noPurposes: CustodyView = {
  catalogue: [],
  backends: [],
  keys: [],
  migrations: [],
  purposes: [],
  records: [],
  stats: { backends: 0, pausedBackends: 0, keys: 0, deniedKeys: 0, migrationsInFlight: 0 },
};

describe('PurposeCustodyPanel', () => {
  it('says no purposes exist yet instead of rendering an empty list', () => {
    render(<PurposeCustodyPanel data={noPurposes} />);

    expect(screen.getByText('No key purposes yet')).toBeInTheDocument();
    expect(screen.queryByRole('list')).not.toBeInTheDocument();
  });
});
