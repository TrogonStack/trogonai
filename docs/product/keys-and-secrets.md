---
title: Keys & Secrets
aside: false
pageClass: wide-page
product:
  name: Keys & Secrets
  tagline: Keep provider credentials in platform custody, with your own keys if you choose.
  order: 5
  status: planned
  icon: key-round
  color: "#E11D48"
guarantees:
  - icon: eye-off
    title: Agents never see a secret
    text: An agent sends ordinary HTTP. It never receives a token or the right to look one up.
  - icon: ban
    title: Fails closed
    text: When a check, a key or a provider fails, the call stops. There is no plaintext fallback.
  - icon: scroll-text
    title: Nothing secret in logs or streams
    text: Secret values never reach logs or traces, and event streams carry no secrets, ciphertext or wrapped keys.
  - icon: power
    title: Your off switch wins
    text: With your own key, disabling it in your provider stops every use. The platform cannot override it.
  - icon: fingerprint
    title: Every key use is audited
    text: Each key operation leaves one audit record, and the proxy stores its audit record before any credential is used.
  - icon: shield-check
    title: No silent fallbacks
    text: A caller never picks the key backend, the host or the credential, and a failing backend is never swapped for another.
progress:
  - name: Workload identity for platform services
    status: built
    note: Services prove who they are with SPIFFE identities instead of shared tokens.
  - name: Tenant-scoped messaging credentials
    status: built
    note: Callers get NATS credentials bound to their tenant and to the subjects they may use.
  - name: Gateway credential storage and rotation
    status: partial
    note: Webhook secrets and bot tokens are stored in OpenBao, as a prototype inside the gateway.
  - name: Key custody console
    status: partial
    note: Screens to register, bind, pause and migrate keys exist, running against a stand-in backend.
  - name: Secrets service and connection proxy
    status: designed
    note: Credential injection on the way out, so agents never hold a credential.
  - name: Envelope encryption and bring your own key
    status: designed
    note: Managed and dedicated keys, plus AWS KMS, Google Cloud KMS and OpenBao backends.
  - name: OAuth connections to providers
    status: designed
    note: Connect a provider once, and the platform exchanges and refreshes its tokens.
---

<ProductHeader />

Agents call GitHub, Linear and other provider APIs without ever holding the
credential. The platform keeps it in custody, adds it to the request on the way
out and encrypts it at rest under a key you choose, all the way up to a key
in your own cloud account that you can switch off at any time.

::: info Designed, mostly not built yet
This page shows the designed product. A few foundations are running today;
[Where it stands](#where-it-stands) lists what is built and what is not.
:::

## Agents never see the secret

The credential lives only inside the custody boundary. Everything on the
agent's side of it is plain HTTP.

<SecretFlow />

## Keys inside keys

Every secret is sealed in layers, the way a letter goes in an envelope that
goes in a safe. Each layer only opens the one directly inside it.

<KeyEnvelope />

## Choose who holds the key

Start on a platform-managed key and move to a key you own when you need to.
Moving re-wraps your existing secrets under the new key, with both keys
available until the switch is complete.

<KeyCustodyTiers />

## A key's life

A key moves through explicit states, and each state says exactly what it can
still do.

<KeyLifecycle />

## What we promise

<FeatureGrid :items="$frontmatter.guarantees" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

## Learn more

**Understand the design**

- [Secret Management](/architecture/secret-management)
- [Key Management](/architecture/key-management)
- [Key Custody](/architecture/key-custody)
- [Key States](/architecture/key-states)
- [Third-Party Connections](/architecture/third-party-connections)

**Set it up**

- [Bring Your Own AWS KMS Key](/how-to/bring-your-own-aws-kms-key)
- [Bring Your Own Google Cloud KMS Key](/how-to/bring-your-own-google-cloud-kms-key)
- [Bring Your Own OpenBao](/how-to/bring-your-own-openbao)
- [Migrate Between Key Backends](/how-to/migrate-key-backends)
- [Handle Unusable Keys](/how-to/handle-unusable-keys)

**Look up details**

- [Connection HTTP Proxy](/reference/connection-http-proxy)
- [Supplied Credential Material](/reference/supplied-credential-material)
- [Workload Identity](/reference/workload-identity)
