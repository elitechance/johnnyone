import { provideZoneChangeDetection } from '@angular/core';
import { bootstrapApplication } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { provideIonicAngular } from '@ionic/angular/standalone';
import { GRAPHQL_EXTRA_HEADERS } from '@johnnyone/ui';
import { AppComponent } from './app/app.component';
import { routes } from './app/app.routes';

bootstrapApplication(AppComponent, {
  providers: [
    provideZoneChangeDetection({ eventCoalescing: true }),
    provideRouter(routes),
    provideIonicAngular({ mode: 'md' }),
    {
      provide: GRAPHQL_EXTRA_HEADERS,
      useValue: {
        'x-tenant-id': '00000000-0000-0000-0000-000000000001',
        'x-user-id': '00000000-0000-0000-0000-000000000002',
      },
    },
  ],
});
