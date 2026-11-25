// Node releases whose lib/path.js the connector vetting probes run against, each with the digest of the reviewed
// source. The first is the default release a probe runs on.
export const NODE_PATH_DIGESTS = {
  '24.13.1': 'dd326ecdc2d6ad2025c4991f4b480d76a9f1d52b7f6d0988a5dc0a1d02de5209',
  '20.20.2': '3b5983b011661f0f8a0560a01d023a445d425e5d5fffbc9b6698ab18b9c20ffc',
  '22.14.0': '52123bd5286bda06daaf3d3ed33e9abfe74d3dfa80a6695b14fbba1ec66a881a',
};

export const NODE_RELEASES = Object.keys(NODE_PATH_DIGESTS);
