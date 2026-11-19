// Incremental SHA-256 (FIPS 180-4). WebCrypto only digests a whole buffer, so
// streamed transfers hash with this instead and never hold the full file.

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
  0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
  0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
  0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
  0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
  0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
  0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
  0xc67178f2,
]);

/** A SHA-256 that takes its input in pieces. */
export class Sha256 {
  private readonly state = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  private readonly block = new Uint8Array(64);
  private readonly w = new Uint32Array(64);
  private blockLen = 0;
  private total = 0;
  private finished = false;

  /** An independent hasher holding the same input so far, for taking a digest mid-stream. */
  clone(): Sha256 {
    if (this.finished) throw new Error('Sha256.clone called after digest');
    const copy = new Sha256();
    copy.state.set(this.state);
    copy.block.set(this.block);
    copy.blockLen = this.blockLen;
    copy.total = this.total;
    return copy;
  }

  /** Adds `bytes` to the message. */
  update(bytes: Uint8Array): this {
    if (this.finished) throw new Error('Sha256.update called after digest');
    this.total += bytes.length;
    let at = 0;
    if (this.blockLen > 0) {
      const take = Math.min(64 - this.blockLen, bytes.length);
      this.block.set(bytes.subarray(0, take), this.blockLen);
      this.blockLen += take;
      at = take;
      if (this.blockLen < 64) return this;
      this.compress(this.block, 0);
      this.blockLen = 0;
    }
    for (; at + 64 <= bytes.length; at += 64) this.compress(bytes, at);
    if (at < bytes.length) {
      this.block.set(bytes.subarray(at));
      this.blockLen = bytes.length - at;
    }
    return this;
  }

  /** The digest as lowercase hex. The hasher cannot be updated afterwards. */
  digestHex(): string {
    if (this.finished) throw new Error('Sha256.digestHex called twice');
    const bitsHigh = Math.floor(this.total / 0x20000000);
    const bitsLow = (this.total * 8) >>> 0;
    const pad = new Uint8Array(this.blockLen < 56 ? 64 - this.blockLen : 128 - this.blockLen);
    pad[0] = 0x80;
    const view = new DataView(pad.buffer);
    view.setUint32(pad.length - 8, bitsHigh);
    view.setUint32(pad.length - 4, bitsLow);
    this.update(pad);
    this.finished = true;
    let hex = '';
    for (const word of this.state) hex += word.toString(16).padStart(8, '0');
    return hex;
  }

  private compress(bytes: Uint8Array, at: number): void {
    const w = this.w;
    for (let i = 0; i < 16; i++) {
      const j = at + i * 4;
      w[i] = (bytes[j] << 24) | (bytes[j + 1] << 16) | (bytes[j + 2] << 8) | bytes[j + 3];
    }
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15];
      const b = w[i - 2];
      const s0 = ((a >>> 7) | (a << 25)) ^ ((a >>> 18) | (a << 14)) ^ (a >>> 3);
      const s1 = ((b >>> 17) | (b << 15)) ^ ((b >>> 19) | (b << 13)) ^ (b >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    const s = this.state;
    let a = s[0];
    let b = s[1];
    let c = s[2];
    let d = s[3];
    let e = s[4];
    let f = s[5];
    let g = s[6];
    let h = s[7];
    for (let i = 0; i < 64; i++) {
      const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
      const ch = (e & f) ^ (~e & g);
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0;
      const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) | 0;
      h = g;
      g = f;
      f = e;
      e = (d + t1) | 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) | 0;
    }
    s[0] = (s[0] + a) | 0;
    s[1] = (s[1] + b) | 0;
    s[2] = (s[2] + c) | 0;
    s[3] = (s[3] + d) | 0;
    s[4] = (s[4] + e) | 0;
    s[5] = (s[5] + f) | 0;
    s[6] = (s[6] + g) | 0;
    s[7] = (s[7] + h) | 0;
  }
}
