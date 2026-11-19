import { createHash } from 'node:crypto';

import { describe, expect, it } from 'vitest';

import { Sha256 } from '../src/sha256.ts';

const enc = new TextEncoder();

function hex(...pieces: Uint8Array[]): string {
  const hash = new Sha256();
  for (const piece of pieces) hash.update(piece);
  return hash.digestHex();
}

describe('Sha256', () => {
  it('matches the FIPS 180-4 vectors', () => {
    expect(hex(new Uint8Array(0))).toBe('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
    expect(hex(enc.encode('abc'))).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
    expect(hex(enc.encode('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq'))).toBe(
      '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1',
    );
    expect(
      hex(
        enc.encode(
          'abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu',
        ),
      ),
    ).toBe('cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1');
  });

  it('hashes a million a in uneven pieces', () => {
    const hash = new Sha256();
    const piece = new Uint8Array(997).fill(0x61);
    let left = 1_000_000;
    while (left > 0) {
      const take = Math.min(piece.length, left);
      hash.update(piece.subarray(0, take));
      left -= take;
    }
    expect(hash.digestHex()).toBe('cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0');
  });

  it('agrees with node:crypto at every split around the block boundary', () => {
    const data = Uint8Array.from({ length: 300 }, (_, i) => (i * 7 + 3) % 256);
    for (let len = 0; len <= 140; len++) {
      const want = createHash('sha256').update(data.subarray(0, len)).digest('hex');
      for (const split of [0, 1, 55, 56, 63, 64, 65]) {
        const at = Math.min(split, len);
        expect(hex(data.subarray(0, at), data.subarray(at, len))).toBe(want);
      }
    }
  });

  it('refuses use after digest', () => {
    const hash = new Sha256();
    hash.digestHex();
    expect(() => hash.update(new Uint8Array(1))).toThrow();
    expect(() => hash.digestHex()).toThrow();
  });
});
