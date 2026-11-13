// Number tokens the generator feeds through the pinned `lE` read path, as raw JSON text because the text is what
// separates `JSON.parse` from a parser that keeps integers exact. The recorded output is the text `JSON.stringify`
// prints for the settings `lE` returns, which is what the Rust port has to reproduce byte for byte.

export const NUMBER_TOKENS = [
  // Zero in every spelling. JSON.stringify prints negative zero as 0.
  '0', '-0', '0.0', '-0.0', '0e0', '-0e0', '0E-5', '1e-400', '-1e-400', '5e-325',
  // Whole numbers around 2^53, where a double stops holding every integer.
  '1', '-1', '1.0', '-1.0', '100e-2', '1e2', '1E2', '12e-1',
  '9007199254740991', '9007199254740992', '9007199254740993', '9007199254740994', '9007199254740995',
  '-9007199254740991', '-9007199254740992', '-9007199254740993', '-9007199254740995',
  '9007199254740992.0', '9.007199254740992e15', '9007199254740993.0', '9007199254740992.5', '9007199254740993.5',
  '4503599627370497', '4503599627370496.5', '4503599627370497.5',
  // 2^53 to 2^64: the integer rounds to a double and prints with zeros after the significant digits.
  '12345678901234567890', '-12345678901234567890', '12345678901234567168', '12345678901234567000',
  '10000000000000000', '10000000000000001', '99999999999999999', '123456789012345678', '1e16', '1.2345678901234567e19',
  '9223372036854775806', '9223372036854775807', '9223372036854775808', '9223372036854775809', '9223372036854774784',
  '-9223372036854775807', '-9223372036854775808', '-9223372036854775809', '-9223372036854774784',
  '18446744073709549568', '18446744073709550592', '18446744073709551614', '18446744073709551615',
  '18446744073709551616', '18446744073709551617',
  // 2^64 to 1e21: JavaScript still prints digits, never an exponent.
  '18446744073709553664', '20000000000000000000', '123456789012345678901', '999999999999999999999',
  '99999999999999991611392', '100000000000000000000', '-100000000000000000000', '-18446744073709551616',
  // 1e21 and above: JavaScript switches to exponent form with an explicit plus sign.
  '1000000000000000000000', '1e21', '1e+21', '1E21', '-1e21', '1.5e21', '123456789012345678901234567890',
  '1e22', '1e23', '9.5e21', '1.5e300', '1e308', '1.7976931348623157e308', '1e100', '1e-100',
  // Beyond the double range the number becomes Infinity, which JSON.stringify prints as null.
  '1e309', '1e400', '-1e400', '1.7976931348623159e308',
  // Fractions: shortest digits, halfway cases and long mantissas.
  '0.1', '0.2', '0.30000000000000004', '0.1e1', '1.5', '-1.5', '4.35', '123456789.123456789', '0.1234567890123456789',
  '1.00000000000000011102230246251565404236316680908203125',
  '1.00000000000000011102230246251565404236316680908203126',
  '1.00000000000000011102230246251565404236316680908203124',
  '0.1000000000000000055511151231257827021181583404541015625',
  '2.2250738585072011e-308', '2.2250738585072014e-308', '2.225073858507201e-308', '5e-324', '4.9e-324', '2.4703282292062327e-324',
  '2.4703282292062328e-324', '1.7976931348623158e308', '123456789012345678.9', '8.41e21',
  // Small magnitudes: decimal down to 1e-6, exponent form below.
  '0.00001', '1e-5', '0.00001234', '0.000001', '1e-6', '1.5e-6', '9.99e-6', '0.0000015', '1e-7', '1.5e-7', '1e-9', '1e-10', '1.234e-10',
  '0.000000123',
  '1e-15', '1e-300',
];

// One document per token, then the same tokens inside containers the read path keeps whole.
export const NUMBER_DOCUMENTS = [
  ...NUMBER_TOKENS.map((token) => `{"n":${token}}`),
  `{"list":[${NUMBER_TOKENS.slice(0, 40).join(',')}]}`,
  '{"custom":{"a":9007199254740993.5,"deep":{"b":[12345678901234567890,{"c":1e300},-0,2.0]}}}',
  '{"cleanupPeriodDays":30.0}',
  '{"cleanupPeriodDays":3e1}',
  '{"cleanupPeriodDays":9007199254740993}',
  '{"cleanupPeriodDays":1e21}',
  '{"cleanupPeriodDays":-0}',
  '{"cleanupPeriodDays":1.5}',
  '{"feedbackSurveyRate":1e-1}',
  '{"feedbackSurveyRate":0.30000000000000004}',
  '{"feedbackSurveyRate":1e400}',
  '{"n":1,"n":12345678901234567890}',
];
