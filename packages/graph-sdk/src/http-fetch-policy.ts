import { lookup } from "node:dns";
import { isIP, type LookupFunction } from "node:net";

import ipaddr from "ipaddr.js";
import { Agent, type Dispatcher } from "undici";

const denied = (): Error => new Error("httpFetch destination denied by egress policy");

/** Positive public-address policy; reject mapped/translated IPv6 and special ranges. */
export const isPublicHttpAddress = (address: string): boolean => {
  if (isIP(address) === 0) return false;
  const parsed = ipaddr.parse(address);
  if (parsed.range() !== "unicast") return false;
  // IPv6 destinations must be in the global unicast allocation, not transition space.
  return parsed.kind() === "ipv4" || parsed.match(ipaddr.parseCIDR("2000::/3"));
};

/** Validate the same canonical URL that will be passed to the transport. */
export const authorizeHttpFetchUrl = (
  raw: string,
  allowedOrigins: readonly string[] | undefined,
  channelConfigured: boolean
): string => {
  const url = new URL(raw);
  if (
    !["http:", "https:"].includes(url.protocol) || url.username || url.password ||
    url.port !== ""
  ) throw denied();

  const hostname = url.hostname.replace(/^\[|\]$/g, "");
  const name = hostname.toLowerCase().replace(/\.$/, "");
  if (name === "localhost" || name.endsWith(".localhost")) throw denied();
  if (isIP(hostname) !== 0 && !isPublicHttpAddress(hostname)) throw denied();

  if (channelConfigured || allowedOrigins !== undefined) {
    if (url.protocol !== "https:" || allowedOrigins === undefined) throw denied();
    const origins = allowedOrigins.map((origin) => {
      const candidate = new URL(origin);
      if (
        candidate.protocol !== "https:" || candidate.username || candidate.password ||
        candidate.port || candidate.pathname !== "/" || candidate.search || candidate.hash
      ) throw denied();
      return candidate.origin;
    });
    if (!origins.includes(url.origin)) throw denied();
  }
  return url.href;
};

/** Validate all answers and give those exact addresses to the socket, with no second lookup. */
export const publicHttpLookup: LookupFunction = (hostname, options, callback) => {
  lookup(hostname, { all: true, verbatim: true }, (error, addresses) => {
    if (error) return callback(error, "");
    if (addresses.length === 0 || addresses.some(({ address }) => !isPublicHttpAddress(address))) {
      return callback(denied(), "");
    }
    const family = Number(options.family ?? 0);
    const eligible = addresses.filter((address) => family === 0 || address.family === family);
    const first = eligible[0];
    if (first === undefined) return callback(denied(), "");
    if (options.all) callback(null, eligible);
    else callback(null, first.address, first.family);
  });
};

/** Per-request agent: no ambient proxy/dispatcher and no connection reuse across invocations. */
export const createHttpFetchDispatcher = (): Dispatcher =>
  new Agent({ connect: { lookup: publicHttpLookup } });
