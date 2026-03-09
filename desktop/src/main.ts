import { provideZoneChangeDetection } from '@angular/core';
import { bootstrapApplication } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { provideIonicAngular } from '@ionic/angular/standalone';
import { AppComponent } from './app/app.component';
import { routes } from './app/app.routes';

function forceDarkMode(): void {
  const root = document.documentElement;
  root.classList.add('dark', 'ion-palette-dark');
  root.style.setProperty('color-scheme', 'dark');

  const applyBodyClass = () => document.body.classList.add('dark', 'ion-palette-dark');
  if (document.body) {
    applyBodyClass();
  } else {
    document.addEventListener('DOMContentLoaded', applyBodyClass, { once: true });
  }
}

forceDarkMode();

bootstrapApplication(AppComponent, {
  providers: [
    provideZoneChangeDetection({ eventCoalescing: true }),
    provideRouter(routes),
    provideIonicAngular({ mode: 'md' }),
  ],
});
