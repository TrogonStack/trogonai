{{- define "sandbox-kubevirt-worker.fullname" -}}
{{- printf "%s-sandbox-kubevirt-worker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-kubevirt-worker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: worker-kubevirt
app.kubernetes.io/name: sandbox-kubevirt-worker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "sandbox-kubevirt-worker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "sandbox-kubevirt-worker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
