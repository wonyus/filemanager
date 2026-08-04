import { beforeEach, describe, expect, it, vi } from "vitest";

const commandMocks = vi.hoisted(() => ({
  listEntries: vi.fn(),
  listBuckets: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  commands: commandMocks,
  formatCommandError: (value: unknown) =>
    value instanceof Error ? value.message : "The operation failed.",
}));

import { useAppStore } from "./appStore";

const location = {
  profileId: "profile-1",
  bucket: "bucket-one",
  prefix: "photos/",
};

const listing = {
  schemaVersion: 1,
  requestGeneration: 1,
  location,
  entries: [],
  isComplete: true,
};

describe("Explorer request state", () => {
  beforeEach(() => {
    commandMocks.listEntries.mockReset();
    commandMocks.listBuckets.mockReset();
    useAppStore.setState({
      selectedProfileId: "profile-1",
      location: null,
      listing: null,
      buckets: [],
      listingError: null,
      bucketError: null,
      listingGeneration: 0,
      profileSelectionGeneration: 0,
      loading: false,
      error: null,
    });
  });

  it("keeps a failed page distinct from an empty prefix and preserves loaded rows", async () => {
    useAppStore.setState({ location, listing });
    commandMocks.listEntries.mockRejectedValueOnce(
      new Error("Access denied for the next page"),
    );

    await useAppStore.getState().listEntries(location, "next-page");

    const state = useAppStore.getState();
    expect(state.listing).toBe(listing);
    expect(state.listingError).toBe("Access denied for the next page");
    expect(state.bucketError).toBeNull();
    expect(state.loading).toBe(false);
  });

  it("ignores a late listing result after navigation", async () => {
    let resolveFirst!: (value: typeof listing) => void;
    const first = new Promise<typeof listing>((resolve) => {
      resolveFirst = resolve;
    });
    const nextLocation = { ...location, prefix: "documents/" };
    commandMocks.listEntries.mockReturnValueOnce(first).mockResolvedValueOnce({
      ...listing,
      requestGeneration: 2,
      location: nextLocation,
    });

    const firstRequest = useAppStore.getState().listEntries(location);
    const secondRequest = useAppStore.getState().listEntries(nextLocation);
    resolveFirst(listing);
    await Promise.all([firstRequest, secondRequest]);

    const state = useAppStore.getState();
    expect(state.location).toEqual(nextLocation);
    expect(state.listing?.location).toEqual(nextLocation);
    expect(state.listingError).toBeNull();
  });

  it("exposes a bucket request failure separately", async () => {
    commandMocks.listBuckets.mockRejectedValueOnce(
      new Error("Bucket access denied"),
    );
    await useAppStore.getState().listBuckets("profile-1");

    const state = useAppStore.getState();
    expect(state.bucketError).toBe("Bucket access denied");
    expect(state.listingError).toBeNull();
  });
});
