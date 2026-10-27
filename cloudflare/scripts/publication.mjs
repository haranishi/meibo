import { planFingerprint, validatePreflight } from "./preflight.mjs";
import { ProviderError } from "./providers.mjs";

/** Saved reports are output only. Production IO always performs fresh provider
 * queries and a fresh source/bundle/image/boot verification before every gate. */
export async function publication(plan, secrets, { expectedAccountId, mode, io }) {
  if (!/^[a-f0-9]{32}$/.test(expectedAccountId || "") || expectedAccountId !== plan.accountId
    || !["check", "publish"].includes(mode)) throw new ProviderError("invalid_input");
  async function gate(currentPlan, registry) {
    const checkedAtMs = Date.now();
    const providers = await io.collect(currentPlan, secrets);
    const artifacts = await io.verify(plan, secrets);
    let image = artifacts.image;
    if (registry) {
      if (registry.configDigest !== artifacts.imageConfigDigest || registry.digest !== currentPlan.imageDigest) throw new ProviderError("artifact");
      image = { ...image, digest: registry.digest };
    }
    const report = { version: 1, kind: "live", checkedAtMs, planSha256: planFingerprint(currentPlan, secrets),
      ...providers, image, bundle: artifacts.bundle };
    await io.record(report, currentPlan);
    const result = validatePreflight({ plan: currentPlan, report, secrets, expectedAccountId, kind: "live" });
    return { result, checkedAtMs };
  }
  await gate(plan);
  if (mode === "check") return { ok: true, deployed: false, deploymentAuthorized: false };
  const registry = await io.push(plan);
  if (!/^sha256:[a-f0-9]{64}$/.test(registry.digest)) throw new ProviderError("artifact");
  const deployedPlan = { ...plan, imageDigest: registry.digest };
  // A registry may use a different manifest serialization from the local OCI
  // layout. Bind its verified config to a fresh boot, then re-run the same gate.
  const gateProof = await gate(deployedPlan, registry);
  try {
    const deployed = await io.install(deployedPlan, secrets, registry, gateProof.checkedAtMs);
    await io.recordDeployment({ ...deployed, sourceRevision: deployedPlan.sourceRevision,
      bundleSha256: deployedPlan.bundleSha256, imageDigest: deployedPlan.imageDigest,
      accountId: expectedAccountId, workerName: deployedPlan.workerName, origin: deployedPlan.origin });
  }
  catch (error) {
    const failure = error instanceof ProviderError ? error : new ProviderError("publication");
    failure.installAttempted = true; throw failure;
  }
  return { ok: true, deployed: true, deploymentAuthorized: false };
}
