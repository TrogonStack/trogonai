import { AuthorizationScheme, ProviderAccessScope } from '@trogonai/proto';
import { useMemo } from 'react';

import { RadioCards, SelectField, TextField, type RadioCardOption, type SelectOption } from '@/components/field';

import { authorizationLabels, selectableAuthorizationSchemes } from '../domain/labels';
import { checkBindingValues, type BindingFormValues, type BindingScopeContext } from '../forms/binding';

const authorizationOptions: readonly SelectOption<number>[] = [
  { value: AuthorizationScheme.UNSPECIFIED, label: 'Not set' },
  ...selectableAuthorizationSchemes.map((scheme) => ({ value: scheme as number, label: authorizationLabels[scheme] })),
];

const kindOptions: readonly RadioCardOption<BindingFormValues['authenticationKind']>[] = [
  { value: 'stored-api-key', label: 'Stored API key', description: 'A secret reference resolved at request time.' },
  {
    value: 'deployment-no-credential',
    label: 'No credential',
    description: 'For a deployment-local endpoint that needs none.',
  },
  { value: 'gateway', label: 'Gateway', description: 'Authenticate through the gateway.' },
];

const gatewayModeOptions: readonly SelectOption<BindingFormValues['gatewayMode']>[] = [
  { value: 'configured-key', label: 'Configured key' },
  { value: 'override-key', label: 'Override key' },
  { value: 'passthrough', label: 'Passthrough' },
  { value: 'no-authentication', label: 'No authentication' },
];

type Errors = Partial<Record<keyof BindingFormValues, string>>;

export function bindingErrors(value: BindingFormValues, context: BindingScopeContext): Errors {
  const errors: Errors = {};
  checkBindingValues(
    value,
    context,
    (path, message) => {
      const key = path[path.length - 1] as keyof BindingFormValues;
      errors[key] ??= message;
    },
    [],
  );
  return errors;
}

interface BindingFieldsProps {
  idPrefix: string;
  value: BindingFormValues;
  context: BindingScopeContext;
  onChange: (value: BindingFormValues) => void;
  showErrors: boolean;
  lockedBindingId?: boolean;
}

export function BindingFields({ idPrefix, value, context, onChange, showErrors, lockedBindingId }: BindingFieldsProps) {
  const errors = useMemo(() => (showErrors ? bindingErrors(value, context) : {}), [showErrors, value, context]);
  const patch = (next: Partial<BindingFormValues>) => onChange({ ...value, ...next });

  const secretVisible =
    value.authenticationKind === 'stored-api-key' ||
    (value.authenticationKind === 'gateway' &&
      (value.gatewayMode === 'configured-key' || value.gatewayMode === 'override-key'));
  const authorizationVisible =
    value.authenticationKind === 'stored-api-key' ||
    (value.authenticationKind === 'gateway' && value.gatewayMode !== 'no-authentication');
  const secretOptional = value.authenticationKind === 'gateway' && value.gatewayMode === 'configured-key';

  return (
    <div className="flex flex-col gap-4">
      <RadioCards
        name={`${idPrefix}-auth-kind`}
        label="Authentication method"
        value={value.authenticationKind}
        options={kindOptions}
        onChange={(authenticationKind) => patch({ authenticationKind })}
        error={errors.authenticationKind}
        columns={3}
        hint={
          context.accessScope === ProviderAccessScope.MANAGED
            ? 'Managed scope: stored API key, or no credential for a deployment-local endpoint.'
            : 'Gateway-only scope requires gateway authentication.'
        }
      />
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <TextField
          id={`${idPrefix}-binding-id`}
          label="Binding id"
          value={value.bindingId}
          onChange={(bindingId) => patch({ bindingId })}
          error={errors.bindingId}
          disabled={lockedBindingId}
          mono
          placeholder="openai-prod-key-1"
          hint="Binding ids are never reused within a connection."
        />
        {value.authenticationKind === 'gateway' ? (
          <SelectField
            id={`${idPrefix}-gateway-mode`}
            label="Gateway mode"
            value={value.gatewayMode}
            options={gatewayModeOptions}
            onChange={(gatewayMode) => patch({ gatewayMode })}
          />
        ) : null}
        {authorizationVisible ? (
          <SelectField
            id={`${idPrefix}-authorization`}
            label="Authorization scheme"
            value={value.authorization}
            options={authorizationOptions}
            onChange={(authorization) => patch({ authorization })}
            error={errors.authorization}
          />
        ) : null}
        {secretVisible ? (
          <TextField
            id={`${idPrefix}-secret-ref`}
            label={secretOptional ? 'Secret reference (optional)' : 'Secret reference'}
            value={value.secretRef}
            onChange={(secretRef) => patch({ secretRef })}
            error={errors.secretRef}
            mono
            placeholder="secret://providers/openai/prod"
            hint="An opaque reference. Credentials are never entered or stored here."
            className={value.authenticationKind === 'gateway' ? undefined : 'md:col-span-2'}
          />
        ) : null}
      </div>
    </div>
  );
}
