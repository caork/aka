import { UserService } from './service';

export function main(): void {
  const svc = new UserService();
  const id = svc.createUser('Ada');
  console.log(svc.findUser(id));
}

main();
