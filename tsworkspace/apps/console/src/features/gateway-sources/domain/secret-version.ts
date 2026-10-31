declare const secretVersionBrand: unique symbol;

export type SecretVersion = number & { readonly [secretVersionBrand]: true };

export function secretVersion(value: number): SecretVersion {
  return value as SecretVersion;
}

export function nextVersion(version: SecretVersion): SecretVersion {
  return (version + 1) as SecretVersion;
}

export function formatVersion(version: SecretVersion): string {
  return `v${version}`;
}
