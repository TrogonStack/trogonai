import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';

import { emptyUsage, USAGE_DAYS } from '../domain/key';
import { UsageChart } from './usage-chart';

it('labels the oldest bar with the same age as its hover text', () => {
  render(<UsageChart usage={emptyUsage()} />);
  expect(screen.getByText(`${USAGE_DAYS - 1} days ago`)).toBeInTheDocument();
  expect(screen.getByTitle(`${USAGE_DAYS - 1}d ago: 0 wraps, 0 unwraps`)).toBeInTheDocument();
});
