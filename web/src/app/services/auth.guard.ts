import { inject } from '@angular/core';
import { CanActivateFn, Router, UrlTree } from '@angular/router';
import { AuthService } from './auth.service';
import { captureReturnUrl } from './return-url-logic';

export const authGuard: CanActivateFn = (_route, state): boolean | UrlTree => {
  const auth = inject(AuthService);
  const router = inject(Router);

  // Recompute against *now* so an in-tab expiry cannot ride a boot-time snapshot.
  if (auth.syncAuthState()) {
    return true;
  }

  const returnUrl = captureReturnUrl(state.url);
  return router.createUrlTree(['/login'], {
    queryParams: returnUrl ? { returnUrl } : undefined,
  });
};
