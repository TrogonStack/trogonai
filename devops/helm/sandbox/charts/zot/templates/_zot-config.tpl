{{/*
  Zot retention lives under storage.retention.
  Helm values retention.defaultPolicy / perTenant map to keepTags criteria below.
*/}}
{{- define "zot.retentionPolicies" -}}
[
{{- $prefix := .Values.repositoryLayout.conventions.snapshotPrefix -}}
{{- $default := .Values.retention.defaultPolicy -}}
{{- $first := true -}}
{{- range $tenantId, $policy := .Values.retention.perTenant }}
{{- if not $first }},{{ end -}}
{{- $first = false -}}
{{- $keepDays := default $default.keepDays $policy.keepDays -}}
{{- $keepLastN := default $default.keepLastN $policy.keepLastN -}}
{{- $hours := mul (int $keepDays) 24 -}}
  {
    "repositories": [{{ printf "%s/%s" $prefix $tenantId | quote }}],
    "deleteReferrers": true,
    "deleteUntagged": true,
    "keepTags": [{
      "mostRecentlyPushedCount": {{ $keepLastN }},
      "pushedWithin": {{ printf "%dh" $hours | quote }}
    }]
  }
{{- end }}
{{- if not $first }},{{ end -}}
{{- $defaultHours := mul (int $default.keepDays) 24 -}}
  {
    "repositories": [{{ printf "%s/**" $prefix | quote }}],
    "deleteReferrers": true,
    "deleteUntagged": true,
    "keepTags": [{
      "mostRecentlyPushedCount": {{ $default.keepLastN }},
      "pushedWithin": {{ printf "%dh" $defaultHours | quote }}
    }]
  }
]
{{- end }}

{{/*
  http.accessControl: platform push on snapshots/**, per-tenant pull-only, anonymous denied.
*/}}
{{- define "zot.accessControlBlock" -}}
{
  "repositories": {
    {{- $prefix := .Values.repositoryLayout.conventions.snapshotPrefix -}}
    {{- $first := true -}}
    {{- range .Values.accessControl.platformIdentity.pushRepositories }}
    {{- if not $first }},{{ end -}}
    {{- $first = false -}}
    {{ . | quote }}: {
      "policies": [{
        "users": [{{ $.Values.accessControl.platformIdentity.serviceAccount | quote }}],
        "actions": ["read", "create", "update"]
      }],
      "defaultPolicy": []
    }
    {{- end }}
    {{- range .Values.accessControl.tenants }}
    {{- if not $first }},{{ end -}}
    {{- $first = false -}}
    {{ printf "%s/%s" $prefix .tenantId | quote }}: {
      "policies": [{
        "users": [{{ .serviceAccount | quote }}],
        "actions": ["read"]
      }],
      "defaultPolicy": []
    }
    {{- end }}
  }
}
{{- end }}

{{- define "zot.syncExtension" -}}
"sync": {
  "enable": true,
  "registries": [
    {{- range $i, $reg := .Values.replication.syncRegistries }}
    {{- if $i }},{{ end }}
    {
      "urls": [{{ $reg.url | quote }}],
      "onDemand": {{ default false $reg.onDemand }},
      "pollInterval": {{ default "6h" $reg.pollInterval | quote }},
      "tlsVerify": {{ default true $reg.tlsVerify }},
      "content": {{ default (list) $reg.content | toJson }}
    }
    {{- end }}
  ]
}
{{- end }}
