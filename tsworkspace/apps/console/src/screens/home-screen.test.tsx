import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { HomeScreen } from './home-screen';

describe('HomeScreen', () => {
  it('renders the TrogonAi Console placeholder', () => {
    render(<HomeScreen />);

    expect(screen.getByText('TrogonAi Console')).toBeInTheDocument();
  });
});
