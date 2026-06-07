package main

import (
	"context"

	"github.com/hashicorp/terraform-plugin-sdk/v2/diag"
	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
)

func Provider() *schema.Provider {
	return &schema.Provider{
		Schema: map[string]*schema.Schema{
			"management_api_endpoint": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Endpoint for the EvergreenShims management API (e.g. http://localhost:9101).",
			},
			"namespace": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "default",
				Description: "Default namespace for resources.",
			},
		},
		ResourcesMap: map[string]*schema.Resource{
			"evergreen_shims_shim_config":  resourceShimConfig(),
			"evergreen_shims_deployment":   resourceShimDeployment(),
		},
		DataSourcesMap: map[string]*schema.Resource{
			"evergreen_shims_status": dataSourceShimStatus(),
		},
		ConfigureContextFunc: providerConfigure,
	}
}

func providerConfigure(ctx context.Context, d *schema.ResourceData) (interface{}, diag.Diagnostics) {
	return &ProviderConfig{
		ManagementAPIEndpoint: d.Get("management_api_endpoint").(string),
		Namespace:             d.Get("namespace").(string),
	}, nil
}

type ProviderConfig struct {
	ManagementAPIEndpoint string
	Namespace             string
}
