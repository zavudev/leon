// The static config is app.json. Android's pushes go through FCM, which needs
// the Firebase project's google-services.json in the build: EAS gives it as the
// file variable GOOGLE_SERVICES_JSON, a local build takes it from this folder,
// and without either the build has no pushes on Android.
const fs = require('node:fs');
const path = require('node:path');

module.exports = ({ config }) => {
  const local = path.join(__dirname, 'google-services.json');
  const file = process.env.GOOGLE_SERVICES_JSON ?? (fs.existsSync(local) ? './google-services.json' : undefined);
  return file ? { ...config, android: { ...config.android, googleServicesFile: file } } : config;
};
