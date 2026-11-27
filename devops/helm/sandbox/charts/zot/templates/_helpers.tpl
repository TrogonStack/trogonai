{{- define "zot.fullname" -}}
{{- printf "%s-zot" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "zot.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: zot
app.kubernetes.io/name: zot
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}
