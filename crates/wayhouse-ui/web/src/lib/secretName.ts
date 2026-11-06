/** The controller's rule: 1 to 63 of a-z, 0-9 and "-", not starting or ending with "-". */
export const validSecretName = (n: string) => /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(n);

/** A suggestion from a config key: `auth_token` becomes `auth-token`. */
export const suggestSecretName = (key: string) =>
  key
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63)
    .replace(/-+$/, "");
