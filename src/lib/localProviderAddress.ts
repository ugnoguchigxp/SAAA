function ipv4Octets(hostname: string): number[] | null {
  const octets = hostname.split(".").map(Number);
  return octets.length === 4
    && octets.every((octet) => Number.isInteger(octet) && octet >= 0 && octet <= 255)
    ? octets
    : null;
}

function isLocalIpv4(octets: number[]): boolean {
  return octets[0] === 10
    || octets[0] === 127
    || (octets[0] === 169 && octets[1] === 254)
    || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
    || (octets[0] === 192 && octets[1] === 168);
}

function isLocalHostname(hostname: string): boolean {
  const labels = hostname.split(".");
  return (labels.length === 1 || hostname.toLowerCase().endsWith(".local"))
    && labels.every((label) => label.length >= 1
      && label.length <= 63
      && /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/.test(label));
}

export function isLocalProviderHost(hostname: string): boolean {
  if (hostname.startsWith("[") && hostname.endsWith("]")) {
    const address = hostname.slice(1, -1);
    return address === "::1"
      || /^(?:fc|fd)[0-9a-f]{2}:/i.test(address)
      || /^fe[89ab][0-9a-f]:/i.test(address);
  }
  const octets = ipv4Octets(hostname);
  return octets ? isLocalIpv4(octets) : isLocalHostname(hostname);
}

export function isDynamicLanHost(value: string): boolean {
  if (!value || value.length > 253 || /[\s/@?#]/.test(value) || value.includes(":")) return false;
  const octets = ipv4Octets(value);
  return octets ? isLocalIpv4(octets) : isLocalHostname(value);
}

export function legacyDynamicLanHost(address: string): string | null {
  try {
    const url = new URL(address);
    return url.protocol === "http:"
      && url.port === "9810"
      && url.pathname === "/"
      && !url.username
      && !url.password
      && !url.search
      && !url.hash
      && isDynamicLanHost(url.hostname)
      ? url.hostname
      : null;
  } catch {
    return null;
  }
}
