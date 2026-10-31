import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { PurposeCustodyPanel } from './purpose-custody-panel';

describe('PurposeCustodyPanel', () => {
  it('says no purposes exist yet instead of rendering an empty list', () => {
    render(<PurposeCustodyPanel purposes={[]} />);

    expect(screen.getByText('No key purposes yet')).toBeInTheDocument();
    expect(screen.queryByRole('list')).not.toBeInTheDocument();
  });
});
