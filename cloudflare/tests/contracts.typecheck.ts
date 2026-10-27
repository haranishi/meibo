import type { DurableObject } from "cloudflare:workers";
import type {
  ContainerPort,
  ContainerRuntime,
  Env,
  ValidatedPublicOrigin,
  ValidatedRequest,
} from "../src/contracts.ts";

type Assert<T extends true> = T;
type Extends<T, U> = [T] extends [U] ? true : false;
type InstalledState = ConstructorParameters<typeof DurableObject>[0];
type InstalledContainer = NonNullable<InstalledState["container"]>;
type InstalledStartOptions = NonNullable<Parameters<InstalledContainer["start"]>[0]>;

// These assertions compile against installed Workers declarations, without runtime stubs.
type RuntimeUsesBuiltInAPI = Assert<Extends<InstalledContainer, ContainerRuntime>>;
type PortUsesBuiltInAPI = Assert<Extends<ReturnType<InstalledContainer["getTcpPort"]>, ContainerPort>>;
type StartOptionsUseBuiltInAPI = Assert<Extends<
  Parameters<ContainerRuntime["start"]>[0],
  InstalledStartOptions
>>;
type NamespaceUsesBuiltInAPI = Assert<Extends<Env["MEIBO_CONTAINER"], DurableObjectNamespace>>;
type SecretsMayBeAbsent = Assert<Extends<undefined, Env["MEIBO_DB_CONTROL_URL"]>>;
type OriginIsExplicit = Assert<Extends<ValidatedRequest["origin"], ValidatedPublicOrigin>>;
type AllowedMethods = Assert<Extends<ValidatedRequest["method"], "GET" | "POST">>;

// @ts-expect-error Raw requests have not passed the public boundary.
type RawRequestIsRejected = Assert<Extends<Request, ValidatedRequest>>;
// @ts-expect-error Runtime contracts do not expose a forced stop path.
type ForcedStopIsUnavailable = ContainerRuntime["destroy"];

export type ContractAssertions = [
  RuntimeUsesBuiltInAPI,
  PortUsesBuiltInAPI,
  StartOptionsUseBuiltInAPI,
  NamespaceUsesBuiltInAPI,
  SecretsMayBeAbsent,
  OriginIsExplicit,
  AllowedMethods,
  RawRequestIsRejected,
  ForcedStopIsUnavailable,
];
