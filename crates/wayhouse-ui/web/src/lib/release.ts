import { useQuery } from "@tanstack/react-query";
import { getRelease } from "../api";

export const RELEASE_KEY = ["release"] as const;

/** The feed is checked once a day by the backend, so the page asks rarely. */
export const useRelease = () =>
  useQuery({ queryKey: RELEASE_KEY, queryFn: getRelease, staleTime: 10 * 60_000, retry: false });

const DISMISSED = "wayhouse.release.dismissed";

/** The dismissed version lives in this browser only; storage may be blocked, so every access is guarded. */
export function dismissedVersion(): string | null {
  try {
    return localStorage.getItem(DISMISSED);
  } catch {
    return null;
  }
}

export function dismissVersion(version: string) {
  try {
    localStorage.setItem(DISMISSED, version);
  } catch {
    /* not remembered; the badge returns on reload */
  }
}

/** Only an https link is rendered, so a hostile feed entry cannot become a `javascript:` link. */
export function safeNotesUrl(url: string): string | null {
  try {
    return new URL(url).protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}
