import { inject } from '@angular/core';
import { CanActivateFn, Router, UrlTree } from '@angular/router';
import { AuthService } from './auth.service';

export const authGuard: CanActivateFn = (_route, state): boolean | UrlTree => {
  const auth = inject(AuthService);
  const router = inject(Router);

  // Recompute against *now* so an in-tab expiry cannot ride a boot-time snapshot.
  if (auth.syncAuthState()) {
    return true;
  }

  return router.createUrlTree(['/login'], {
    queryParams: state.url && state.url !== '/' ? { returnUrl: state.url } : undefined,
  });
};
