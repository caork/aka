import { connect, Database } from './db';

export class UserService {
  private db: Database;

  constructor() {
    this.db = connect('demo://local');
  }

  createUser(name: string): string {
    const id = `u_${name.toLowerCase()}`;
    this.db.put(id, name);
    return id;
  }

  findUser(id: string): string | undefined {
    return this.db.get(id);
  }
}
