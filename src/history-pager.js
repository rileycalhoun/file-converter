// Own pagination state independently of the DOM so overlapping refreshes cannot
// append stale pages or restore entries deleted by a newer history refresh.
export class HistoryPager {
  constructor(fetchPage, pageSize = 50) {
    this.fetchPage = fetchPage;
    this.pageSize = pageSize;
    this.items = [];
    this.nextCursor = null;
    this.loaded = false;
    this.loading = false;
    this.generation = 0;
    this.version = 0;
    this.mutations = new Map();
  }

  async load({ reset = false } = {}) {
    if (!reset && (this.loading || (this.loaded && !this.nextCursor))) return null;
    const generation = reset ? ++this.generation : this.generation;
    const version = this.version;
    this.loading = true;
    try {
      const page = await this.fetchPage({ after: reset ? null : this.nextCursor, pageSize: this.pageSize });
      if (generation !== this.generation) return null;
      const previous = reset ? [] : this.items;
      const known = new Set(previous.map((entry) => entry.id));
      const entries = page.entries.map((entry) => {
        const mutation = this.mutations.get(entry.id);
        return mutation && mutation.version > version ? mutation.entry : entry;
      }).filter((entry) => {
        if (!entry) return false;
        if (known.has(entry.id)) return false;
        known.add(entry.id);
        return true;
      });
      this.items = [...previous, ...entries];
      this.nextCursor = page.nextCursor;
      this.loaded = true;
      for (const [id, mutation] of this.mutations) {
        if (mutation.version <= version) this.mutations.delete(id);
      }
      return { entries, reset };
    } catch (error) {
      if (generation !== this.generation) return null;
      throw error;
    } finally {
      if (generation === this.generation) this.loading = false;
    }
  }

  update(entry) {
    this.mutations.set(entry.id, { version: ++this.version, entry });
    const index = this.items.findIndex((item) => item.id === entry.id);
    if (index !== -1) this.items[index] = entry;
  }

  remove(id) {
    this.mutations.set(id, { version: ++this.version, entry: null });
    this.items = this.items.filter((entry) => entry.id !== id);
    // The cursor describes the sort boundary, even if its row has been deleted.
  }
}
