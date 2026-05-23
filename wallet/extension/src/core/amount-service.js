const MAX_U64 = 18_446_744_073_709_551_615n;

export function normalizeBaseUnitDecimals(decimals, label = 'decimals') {
  const value = Number(decimals);
  if (!Number.isSafeInteger(value) || value < 0 || value > 18) {
    throw new Error(`${label} must be an integer between 0 and 18`);
  }
  return value;
}

export function parseDecimalBaseUnits(value, decimals = 9, label = 'Amount') {
  const unitDecimals = normalizeBaseUnitDecimals(decimals);
  let text = String(value ?? '').trim();
  if (text.startsWith('.')) text = `0${text}`;
  if (!text || !/^\d+(?:\.\d*)?$/.test(text)) {
    throw new Error(`${label} must be a decimal amount`);
  }

  const [wholeRaw, fractionRaw = ''] = text.split('.');
  if (fractionRaw.length > unitDecimals) {
    throw new Error(`${label} supports at most ${unitDecimals} decimal places`);
  }

  const scale = 10n ** BigInt(unitDecimals);
  const whole = BigInt(wholeRaw || '0');
  const fraction = unitDecimals === 0
    ? 0n
    : BigInt((fractionRaw + '0'.repeat(unitDecimals)).slice(0, unitDecimals) || '0');
  const units = whole * scale + fraction;
  if (units > MAX_U64) {
    throw new Error(`${label} is too large`);
  }
  return units;
}

export function parsePositiveDecimalBaseUnits(value, decimals = 9, label = 'Amount') {
  const units = parseDecimalBaseUnits(value, decimals, label);
  if (units <= 0n) {
    throw new Error(`${label} must be greater than zero`);
  }
  return units;
}

export function baseUnitsToDecimalString(value, decimals = 9) {
  const unitDecimals = normalizeBaseUnitDecimals(decimals);
  const units = typeof value === 'bigint' ? value : BigInt(String(value || 0));
  const scale = 10n ** BigInt(unitDecimals);
  const whole = units / scale;
  if (unitDecimals === 0) return whole.toString();
  const fraction = (units % scale).toString().padStart(unitDecimals, '0').replace(/0+$/, '');
  return fraction ? `${whole.toString()}.${fraction}` : whole.toString();
}
