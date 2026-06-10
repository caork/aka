export class Database {
  private rows: Map<string, string> = new Map();

  put(key: string, value: string): void {
    this.rows.set(key, value);
  }

  get(key: string): string | undefined {
    return this.rows.get(key);
  }
}

export function connect(url: string): Database {
  void url;
  return new Database();
}
