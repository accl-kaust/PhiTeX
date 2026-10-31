// The value of the latest version: a viewer's PDF, loaded again after each
// `settled`. A version that moves while its load is in flight is loaded
// again before the value is given: a caller never gets the load that
// began before the last `settled` (the stale page a `settled` that came
// during the previous version's load left on the screen).

export class Current<T> {
  private readonly load: () => Promise<T | null>;
  private readonly drop: (old: T) => void;
  /** The versions announced, and the one the value is of. */
  private version = 0;
  private valueVersion = -1;
  private value: T | null = null;
  private loading: Promise<T | null> | null = null;

  /** `load` makes the value as it is now (null: none); `drop` frees one replaced. */
  constructor(load: () => Promise<T | null>, drop: (old: T) => void = () => {}) {
    this.load = load;
    this.drop = drop;
  }

  /** A new version is out (a `settled`): the next `get` loads it. */
  moved(): void {
    this.version++;
  }

  /** The value of the latest version, loaded until the one it is of is the latest (null: none). */
  async get(): Promise<T | null> {
    for (;;) {
      if (this.value !== null && this.valueVersion === this.version) return this.value;
      if (!this.loading) {
        const v = this.version;
        this.loading = this.load()
          .then((x) => {
            if (x !== null) {
              if (this.value !== null && this.value !== x) this.drop(this.value);
              this.value = x;
              this.valueVersion = v;
            }
            return x;
          })
          .finally(() => (this.loading = null));
      }
      const x = await this.loading;
      if (x === null) return null;
      if (this.valueVersion === this.version) return this.value;
    }
  }
}
