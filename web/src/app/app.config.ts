import { ApplicationConfig, inject, provideAppInitializer, provideZoneChangeDetection } from '@angular/core';
import { provideRouter, TitleStrategy } from '@angular/router';
import { provideIonicAngular } from '@ionic/angular/standalone';
import { GRAPHQL_API_URL } from '@johnnyone/ui';
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
  ],
};
