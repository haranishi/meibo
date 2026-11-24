import type { DurableObject } from "cloudflare:workers";
import type {
  AlarmStorage,
  CacheLike,
  ContainerPort,
  ContainerRuntime,
  DemoContainerNamespace,
  DemoExecutionContext,
  DemoWorkerEnv,
  Env,
  RateLimitBinding,
} from "../src/contracts.ts";

type Assert<T extends true> = T;
type Extends<T, U> = [T] extends [U] ? true : false;
type InstalledState = ConstructorParameters<typeof DurableObject>[0];
type InstalledContainer = NonNullable<InstalledState["container"]>;
type InstalledStartOptions = NonNullable<Parameters<InstalledContainer["start"]>[0]>;

// 導入済みの @cloudflare/workers-types と wrangler types の型契約をコンパイル時に検証する。
type EnvSatisfiesWorkerEnv = Assert<Extends<Env, DemoWorkerEnv>>;
type RuntimeMatchesBuiltIn = Assert<Extends<InstalledContainer, ContainerRuntime>>;
type PortMatchesBuiltIn = Assert<Extends<ReturnType<InstalledContainer["getTcpPort"]>, ContainerPort>>;
type StartOptionsMatchBuiltIn = Assert<Extends<
  Parameters<ContainerRuntime["start"]>[0],
  InstalledStartOptions
>>;
type StorageMatchesAlarmOnly = Assert<Extends<InstalledState["storage"], AlarmStorage>>;
type RateLimitMatchesBuiltIn = Assert<Extends<RateLimit, RateLimitBinding>>;
type CacheMatchesBuiltIn = Assert<Extends<Cache, CacheLike>>;
type ContextMatchesBuiltIn = Assert<Extends<ExecutionContext, DemoExecutionContext>>;
type NamespaceMatchesBuiltIn = Assert<Extends<Env["DEMO_CONTAINER"], DemoContainerNamespace>>;

// @ts-expect-error AlarmStorage には業務データ書き込み用の put を公開しない。
type StoragePutIsUnavailable = AlarmStorage["put"];
// @ts-expect-error AlarmStorage には SQL 領域を公開しない。
type StorageSqlIsUnavailable = AlarmStorage["sql"];
// @ts-expect-error デモ用コンテナは外部インターネット通信を許可しない。
type InternetCannotBeEnabled = Assert<Extends<{ env: Record<string, string>; enableInternet: true }, Parameters<ContainerRuntime["start"]>[0]>>;

export type ContractAssertions = [
  EnvSatisfiesWorkerEnv,
  RuntimeMatchesBuiltIn,
  PortMatchesBuiltIn,
  StartOptionsMatchBuiltIn,
  StorageMatchesAlarmOnly,
  RateLimitMatchesBuiltIn,
  CacheMatchesBuiltIn,
  ContextMatchesBuiltIn,
  NamespaceMatchesBuiltIn,
  StoragePutIsUnavailable,
  StorageSqlIsUnavailable,
  InternetCannotBeEnabled,
];
