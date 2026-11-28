{{/*
Render a container image reference as <repository>:<tag>@sha256:<digest>.

Arguments:
  image  the values block holding repository, tag and digest
  name   the image name under global.imageRegistry, used when repository is empty
  global the chart's global values
  path   the values path of the block, named in the error when a field is missing

Nothing renders unpinned: a missing tag or digest fails the render.
*/}}
{{- define "trogon-sandbox-library.image" -}}
{{- $image := .image -}}
{{- $repository := $image.repository | default (printf "%s/%s" .global.imageRegistry .name) -}}
{{- $tag := required (printf "%s.tag is required: pin the image to a tag and a digest, see docs/how-to/sandbox/deploy.md" .path) $image.tag -}}
{{- $digest := required (printf "%s.digest is required: pin the image to a tag and a digest, see docs/how-to/sandbox/deploy.md" .path) $image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "%s.digest must be sha256: followed by 64 lowercase hex digits, got %q" .path $digest) -}}
{{- end -}}
{{- printf "%s:%s@%s" $repository $tag $digest -}}
{{- end }}
