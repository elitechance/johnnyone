import { ApplicationConfig, inject, provideAppInitializer, provideZoneChangeDetection } from '@angular/core';
import { provideRouter, TitleStrategy } from '@angular/router';
import { provideIonicAngular } from '@ionic/angular/standalone';
import { GRAPHQL_API_URL, GRAPHQL_AUTH_REFRESH } from '@johnnyone/ui';
import { appRoutes } from './app.routes';
import { AppTitleStrategy } from './app-title-strategy';
import { AuthService } from './services/auth.service';

export const appConfig: ApplicationConfig = {
  providers: [
    provideZoneChangeDetection({ eventCoalescing: true }),
    provideRouter(appRoutes),
    provideIonicAngular(),
    { provide: TitleStrategy, useClass: AppTitleStrategy },
    provideAppInitializer(() =>
      inject(AuthService).startSession(inject(GRAPHQL_API_URL)),
    ),
    // The seam that lets `GraphQLClient` recover from an expired access token
    // without `ui/` ever importing `AuthService`: the library takes a plain
    // callback and does nothing unless one is supplied.
    {
      provide: GRAPHQL_AUTH_REFRESH,
      useFactory: () => {
        const auth = inject(AuthService);
        const apiUrl = inject(GRAPHQL_API_URL);
        return () => auth.ensureFreshToken(apiUrl);
      },
    },
  ],
};
