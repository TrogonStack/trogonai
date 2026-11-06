# Enterprise SPIRE deployment assets

The renderer prepares the Kubernetes profile described in
[the operator guide](../../../docs/how-to/deploy-enterprise-workload-identity.md).
It requires exact release signature verification before emitting deployment
output. An image digest or release workflow source alone cannot pass that gate.

The example describes resource identities and references existing credentials;
it contains no production credentials. Replace its example values with reviewed
deployment inputs. Rendering does not deploy resources or certify enterprise
acceptance.

```sh
mise exec cosign@3.1.4 -- python3 devops/kubernetes/spire/render.py \
  --config deployment.json --output rendered-identity
```

Configuration and policy regressions run locally without an issuer:

```sh
mise exec -- python3 -m unittest discover -s devops/kubernetes/spire -p 'test_*.py'
mise exec go@1.27.3 -- go -C devops/kubernetes/spire/celtest test -count=1 ./...
```

The CEL suite evaluates the generated admission expressions with the CEL version
used by Kubernetes 1.37. Disable the Go test cache so a changed Python renderer
is evaluated. Production acceptance additionally requires API-server
type checking and actual admission attempts in the selected cluster.

The admission baseline applies cluster-wide and allows host privileges only
for the exact generated SPIRE agent profile. It also bounds issuer, agent and
supervisor node selection through pod and binding admission. Privileged system
components need separately reviewed exact profiles and live recovery evidence
before rollout; namespace-wide exemptions are not supported. Existing pods and
PVC/CSI backing storage require independent isolation validation. Follow the
operator guide's infrastructure inventory and enablement gates before applying.

An optional rebuild-input preparer authenticates official release archives and
a pinned distroless base, then emits Dockerfiles, binaries, license and input
verification receipts. It never builds or publishes an image and does not
change the deployment renderer's trust policy:

```sh
mise exec cosign@3.1.4 -- python3 devops/kubernetes/spire/prepare_images.py \
  --architecture arm64 --scratch-root scratch --output authenticated-build-inputs
```

The scratch directory must exist. GitHub's authenticated release-publisher
attestation establishes the archive's release origin and bytes. It does not
establish SLSA build provenance. A downstream image still needs approved build
and signature evidence for its exact digest before deployment.
