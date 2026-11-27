{{- define "spicedb.fullname" -}}
{{- printf "%s-spicedb" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "spicedb.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: spicedb
app.kubernetes.io/name: spicedb
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}
