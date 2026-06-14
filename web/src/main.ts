import { bootstrapApplication } from '@angular/platform-browser';
import { GRAPHQL_API_URL, GRAPHQL_WS_URL, HOST_GRAPHQL_API_URL } from '@johnnyone/ui';
import { appConfig } from './app/app.config';
import { AppComponent } from './app/app.component';
import { getWorkerGraphqlUrl, getWorkerGraphqlWsUrl } from './worker-url';

bootstrapApplication(AppComponent, {
  ...appConfig,
  providers: [
    ...(appConfig.providers ?? []),
    { provide: GRAPHQL_API_URL, useValue: getWorkerGraphqlUrl() },
    { provide: HOST_GRAPHQL_API_URL, useValue: 'http://127.0.0.1:7788/graphql' },
    { provide: GRAPHQL_WS_URL, useValue: getWorkerGraphqlWsUrl() },
  ],
}).catch((err) => console.error(err));
