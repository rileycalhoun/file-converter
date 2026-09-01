export class ConversionGuard {
  #activeToken = null;

  get isActive() {
    return this.#activeToken !== null;
  }

  begin() {
    if (this.#activeToken !== null) return null;
    this.#activeToken = Symbol("conversion");
    return this.#activeToken;
  }

  finish(token) {
    if (token !== this.#activeToken) return false;
    this.#activeToken = null;
    return true;
  }
}
